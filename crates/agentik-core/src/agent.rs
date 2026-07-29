//! # Design Principles
//!
//! Complete separation of tool invocation from tool execution within the agent system.
//! The core agent loop is behaviorally uniform across all agents — hardcoded logic provides
//! only generic capabilities (request–response cycling, lifecycle management, effect application)
//! and never encodes agent-specific behavior, tool selection, or prompt engineering at the
//! structural level. Agent personality and tooling are configured exclusively through the
//! toolset and system prompt, not through code paths.

use std::{sync::Arc, time::Duration, time::UNIX_EPOCH};

use crate::context::ContextProvider;
use crate::message_ext::AgentMessageExt;
use agentik_sdk::model::model_pool::ModelPool;
use agentik_sdk::types::messages::{ContentBlock, Message, Role};
use agentik_sdk::types::ToolDefinition;
use agentik_sdk::types::tools::ToolUse;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{Level, span};
use uuid::Uuid;

use agentik_sdk::types::AgentEvent;

use crate::prompt::system_prompt_builder;

use crate::{
    error::{AgentError, Retryable},
    lifecycle::AgentLifecycle,
    memory::Memory,
    skill::SharedSkillRuntime,
    storage::{AgentSnapshot, AgentSnapshotStorage},
    tools::{ToolRegistration, Toolset},
};

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

/// Internal events that wake the agent's outer [`run`] loop.
///
/// External callers (e.g. the TUI runtime) send these through
/// [`Agent::internal_event_tx`] to drive the agent without holding a
/// reference to the `Agent` struct.
pub enum InternalEvent {
    /// User injected a new message (already in memory via `inject_message`).
    MessageInject(Vec<ContentBlock>),
    /// A background tool task (with the given `tool_use_id`) finished.
    /// Its real result stays in the `TaskEntry` and is read on demand via
    /// `view_task_results` — it is NOT injected into memory.
    BgTaskComplete(String),
    Done,
    /// A tool requested the current session be aborted (e.g. `abort_task`).
    Abort,
    /// External Runtime requests the agent to shut down.
    Shutdown,
    /// Replace the agent's cancellation token with a fresh one.
    /// Sent by `AgentRuntime::cancel()` after cancelling the current
    /// session so that the next session can run normally.
    ResetCancelToken(CancellationToken),
}

pub struct Agent {
    pub(crate) id: Uuid,
    pub(crate) model_pool: Arc<ModelPool>,
    pub(crate) memory: Memory,
    pub(crate) lifecycle: AgentLifecycle,
    pub(crate) toolset: Toolset,
    pub(crate) config: AgentConfig,
    pub(crate) storage: Option<Arc<dyn AgentSnapshotStorage>>,
    pub(crate) token_budget: TokenBudget,
    pub(crate) context_provider: Option<Arc<dyn ContextProvider>>,
    pub(crate) system_prompt_section: Option<String>,
    pub(crate) system_prompt_identity: Option<String>,
    /// Optional active skill workflow. When set, the agent is constrained
    /// to the current step's `allowed_tools` each turn and the step's todo
    /// progress is injected into the system prompt.
    pub(crate) skill_runtime: Option<SharedSkillRuntime>,
    /// Optional event channel for streaming progress to external observers.
    pub agent_event_tx: Option<tokio::sync::mpsc::UnboundedSender<agentik_sdk::types::AgentEvent>>,
    /// Currently selected model name. If None, falls back to round-robin.
    pub(crate) current_model_name: Option<String>,
    /// External cancellation signal, Cloned out to callers so they can
    /// interrupt the agent loop cooperatively.
    pub(crate) cancel_token: CancellationToken,
    pub(crate) internal_event_tx: tokio::sync::mpsc::UnboundedSender<InternalEvent>,
    /// Receiver consumed once by [`run()`]; `None` after that.
    pub(crate) internal_event_rx: Option<tokio::sync::mpsc::UnboundedReceiver<InternalEvent>>,
}

impl Agent {
    pub fn builder() -> crate::agent_builder::AgentBuilder {
        crate::agent_builder::AgentBuilder::new()
    }

    /// Send an event to the optional observation channel.
    fn send_event(&self, event: agentik_sdk::types::AgentEvent) {
        if let Some(tx) = &self.agent_event_tx {
            let _ = tx.send(event);
        }
    }

    /// Switch to a different model by name. No-op if the name is not in the pool.
    pub fn select_model(&mut self, name: &str) {
        if self.model_pool.get_model_by_name(name).is_ok() {
            self.current_model_name = Some(name.to_string());
        }
    }

    /// Returns the currently selected model name, or None for round-robin.
    pub fn current_model(&self) -> Option<&str> {
        self.current_model_name.as_deref()
    }

    /// Returns the agent's unique ID.
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Returns a clone of the internal event sender.
    ///
    /// Used by the sync-to-async bridge (e.g. `runtime`) to
    /// inject [`InternalEvent`]s without holding a reference to the Agent.
    pub fn internal_event_tx(&self) -> tokio::sync::mpsc::UnboundedSender<InternalEvent> {
        self.internal_event_tx.clone()
    }

    /// Wire an event channel for external observation (e.g. TUI, tests).
    pub fn set_agent_event_tx(
        &mut self,
        tx: tokio::sync::mpsc::UnboundedSender<agentik_sdk::types::AgentEvent>,
    ) {
        self.agent_event_tx = Some(tx);
    }

    /// Register a single tool.
    pub fn register_tool(&mut self, registration: ToolRegistration) -> Result<(), AgentError> {
        self.toolset
            .register(registration)
            .map_err(AgentError::Tool)?;
        Ok(())
    }

    /// Register multiple tools at once.
    pub fn register_tools(
        &mut self,
        registrations: Vec<ToolRegistration>,
    ) -> Result<(), AgentError> {
        self.toolset
            .register_all(registrations)
            .map_err(AgentError::Tool)?;
        Ok(())
    }

    /// Override the system prompt identity line.
    pub fn set_system_prompt_identity(&mut self, identity: impl Into<String>) {
        self.system_prompt_identity = Some(identity.into());
    }

    /// Override the system prompt section.
    pub fn set_system_prompt_section(&mut self, section: impl Into<String>) {
        self.system_prompt_section = Some(section.into());
    }

    pub async fn snapshot(&self) -> AgentSnapshot {
        let snapshot = AgentSnapshot {
            snapshot_id: Uuid::new_v4(),
            ts: std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
            agent_id: self.id,
            agent_status: *self.lifecycle.status(),
            memory: self.memory.clone(),
        };

        if let Some(storage) = self.storage.clone() {
            let _ = storage.as_ref().create_snapshot(snapshot.clone()).await;
        }

        snapshot
    }

    pub fn lifecycle_status(&self) -> agentik_types::AgentLifecycleStatus {
        *self.lifecycle.status()
    }

    pub fn is_running(&self) -> bool {
        self.lifecycle.is_running()
    }

    pub fn inject_message(&mut self, user_content: Vec<ContentBlock>) -> Result<(), AgentError> {
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

    /// Replace the cancellation token before the next [`start`] call.
    ///
    /// Because `CancellationToken` is one-shot (once cancelled it stays
    /// cancelled), callers that reuse an `Agent` across multiple runs must
    /// inject a fresh token each time — otherwise a prior cancel would
    /// prevent every subsequent `start()` from entering its loop.
    pub fn set_cancel_token(&mut self, token: CancellationToken) {
        self.cancel_token = token;
    }

    /// Apply a single [`InternalEvent`] against agent state.
    ///
    /// Returns `true` when the event represents new work that should keep
    /// the (outer or session) loop running, and `false` for terminal
    /// control signals (`Abort`, `Shutdown`) that ask the loop to stop.
    async fn apply_internal_event(&mut self, event: InternalEvent) -> bool {
        match event {
            InternalEvent::MessageInject(content) => {
                let _ = self.inject_message(content);
                true
            }
            InternalEvent::BgTaskComplete(id) => {
                // A background task finished. Its real result stays in the
                // `TaskEntry` (read on demand via `view_task_results`) and is
                // NOT injected into memory, to avoid polluting the LLM context.
                // We only: surface the result to the TUI, and leave a
                // lightweight user-message pointer telling the model the task
                // is done and how to fetch the result. The entry itself is kept
                // so `view_task_results` can still read it by id.
                if let Some((name, ok, content)) =
                    self.toolset.finished_task_notification(&id).await
                {
                    self.send_event(agentik_sdk::types::AgentEvent::ToolBackgroundComplete {
                        id: id.clone(),
                        ok,
                    });
                    let note = if ok {
                        format!(
                            "Background task '{name}' (id={id}) finished. \
                             Call `view_task_results` with task_id={id} to read its result."
                        )
                    } else {
                        format!(
                            "Background task '{name}' (id={id}) finished with an error: {content}. \
                             Call `view_task_results` with task_id={id} to read the error."
                        )
                    };
                    let _ = self.memory.remember(Message::user(note));
                }
                true
            }
            InternalEvent::Abort | InternalEvent::Shutdown => {
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
        }
    }

    /// Long-running event loop that drives the agent autonomously.
    ///
    /// Blocks until shut down.  Internal events wake the agent; the agent
    /// runs one "session" (zero or more LLM round-trips) per wake-up and
    /// returns to idle when the LLM produces no tool calls.
    ///
    /// Exit paths:
    /// - [`InternalEvent::Shutdown`] received
    /// - Channel closed (all senders dropped)
    ///
    /// Cancellation is handled inside [`run_session()`]; it interrupts the
    /// current session but the outer loop stays alive to process future
    /// messages. The cancel token is replaced via [`InternalEvent::ResetCancelToken`]
    /// so that subsequent sessions see a fresh (non-cancelled) token.
    pub async fn run(&mut self) {
        let mut rx = self
            .internal_event_rx
            .take()
            .expect("internal_event_rx already consumed by a prior run()");

        loop {
            let event = match rx.recv().await {
                Some(e) => e,
                None => break, // channel closed → shutdown
            };

            let should_run = matches!(
                event,
                InternalEvent::MessageInject(_) | InternalEvent::BgTaskComplete(_)
            );
            let keep_going = self.apply_internal_event(event).await;
            if !keep_going {
                break;
            }

            if should_run {
                self.run_session(&mut rx).await;
            }
        }

        self.lifecycle.set_aborted();
        self.snapshot().await;
    }

    /// Run one "session": a sequence of LLM round-trips until the agent
    /// goes idle, hits an error, or is cancelled.
    ///
    /// Returns when the agent is idle (lifecycle = IDLE) or aborted.
    async fn run_session(&mut self, rx: &mut tokio::sync::mpsc::UnboundedReceiver<InternalEvent>) {
        self.lifecycle.set_running();
        self.send_event(agentik_sdk::types::AgentEvent::LlmResponse(
            "🤖 Agent started".into(),
        ));
        let cancelled = self.cancel_token.clone();

        let mut iteration = 0;
        let mut consecutive_retries = 0;

        loop {
            if iteration >= self.config.max_iterations
                || !self.lifecycle.is_running()
                || cancelled.is_cancelled()
            {
                break;
            }

            iteration += 1;

            // Run one workflow iteration to completion. Only an external
            // cancellation may interrupt it — we deliberately do NOT select on
            // `rx.recv()` here.
            //
            // `agent_workflow` records the assistant's `tool_use` blocks to
            // memory *before* executing the tools, and records the matching
            // `tool_result` blocks only *after* `toolset.execute` returns. If a
            // `BgTaskComplete` / `MessageInject` event were allowed to preempt
            // the workflow mid-execution (as the previous `select! { rx.recv() }`
            // branch did), `tokio::select!` would drop the in-flight
            // `agent_workflow` future, losing the still-pending tool results.
            // The result is an orphaned `tool_use` with no following
            // `tool_result`, which strict providers (Anthropic-compatible /
            // DeepSeek) reject as a 400 `invalid_request_error`.
            //
            // Events that arrive while a workflow is running are buffered in the
            // unbounded channel and drained below, *between* iterations — by
            // which point every tool_use of this turn already has its
            // tool_result safely in memory.
            let result = tokio::select! {
                biased; // cancellation always takes priority
                _ = cancelled.cancelled() => {
                    self.lifecycle.set_aborted();
                    self.snapshot().await;
                    self.send_event(AgentEvent::Error("Task cancelled by user".into()));
                    break;
                }
                result = self.agent_workflow(None) => result,
            };

            let session_done = match result {
                Ok(()) => {
                    consecutive_retries = 0;
                    self.snapshot().await;
                    // `agent_workflow` flips the lifecycle to IDLE when the LLM
                    // produces no tool calls (natural completion). That same flip
                    // makes `is_running()` false here, so we must emit `Done` on
                    // this branch — otherwise the TUI never leaves the Running
                    // state.
                    if !self.lifecycle.is_running() {
                        self.send_event(agentik_sdk::types::AgentEvent::Done);
                        true
                    } else {
                        false
                    }
                }
                Err(AgentError::CompactionRebuild) => {
                    tracing::info!("compaction rebuild, re-entering workflow");
                    continue;
                }
                Err(e) if e.is_retryable() && consecutive_retries < self.config.max_retries => {
                    consecutive_retries += 1;
                    tracing::warn!(
                        "retryable error at iteration {}/{}, retry {}/{}: {e}",
                        iteration,
                        self.config.max_iterations,
                        consecutive_retries,
                        self.config.max_retries
                    );
                    let delay = Duration::from_secs(1) * (1 << (consecutive_retries - 1));
                    tokio::time::sleep(delay).await;
                    let _ = self.memory.remember(Message::user(e.retry_message()));
                    continue;
                }
                Err(e) => {
                    tracing::error!("workflow failed at iteration {iteration}: {e}");
                    self.send_event(AgentEvent::Error(format!("{e}")));
                    self.snapshot().await;
                    self.lifecycle.set_idle();
                    true
                }
            };

            if session_done {
                break;
            }

            // Drain control/notification events that arrived during this
            // iteration. Non-preempting: the workflow already completed and
            // recorded all of its tool_results, so injecting a
            // `BgTaskComplete` or `MessageInject` here cannot orphan a
            // tool_use. A `BgTaskComplete` whose result was already consumed
            // by `wait_task` is a no-op: the task entry is reclaimed by the
            // next `toolset.execute`, so `finished_task_notification` returns
            // `None` and nothing redundant is injected.
            let mut terminal = false;
            while let Ok(event) = rx.try_recv() {
                if !self.apply_internal_event(event).await {
                    // Terminal control signal (Abort / Shutdown / Done).
                    terminal = true;
                    break;
                }
            }
            if terminal {
                break;
            }
        }

        // Post-loop cleanup: ensure the TUI always receives a terminal
        // event and the lifecycle is reset, regardless of how the loop
        // exited.
        //
        // Normally the `cancelled` select! branch sends the Error event and
        // sets the lifecycle to aborted, which then becomes idle here.
        // But when a `ResetCancelToken` InternalEvent arrives via `rx.recv()`
        // before the `cancelled.cancelled()` branch fires (a race in
        // `cancel()`), the loop breaks via the condition check at the top
        // without sending any event or touching the lifecycle.  Detect that
        // case and emit the expected shutdown sequence.
        if cancelled.is_cancelled() && self.lifecycle.is_running() {
            self.lifecycle.set_idle();
            self.send_event(AgentEvent::Error("Task cancelled by user".into()));
        } else if !self.lifecycle.is_running() {
            self.lifecycle.set_idle();
        }
    }

    /// Core agent workflow
    ///
    /// Basic process: build context -> request API -> execute tool calls -> append to memory.
    /// Returns `Err(AgentError::CompactionRebuild)` when a compaction occurred and the
    /// caller should re-enter this method with fresh context.
    async fn agent_workflow(&mut self, retry_feedback: Option<String>) -> Result<(), AgentError> {
        if let Some(feedback) = retry_feedback {
            self.inject_message(vec![ContentBlock::Text { text: feedback }])
                .unwrap();
        }

        // Poll context provider for dynamic injection
        self.poll_context_provider().await;

        let context = self.build_context().await?;

        // If a skill is active, restrict the LLM's toolset to the current
        // step's allowed tools for this turn. The same whitelist is enforced
        // again at execution time below.
        let allowed = self.current_allowed_tools().await;

        self.send_event(agentik_sdk::types::AgentEvent::Requesting);
        let response_message = self.request(context, allowed.as_deref()).await?;

        let last_usage = response_message.usage.clone().unwrap_or_default();

        // Emit LLM text and thinking content for UI observation
        for block in &response_message.content {
            match block {
                ContentBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                    self.send_event(agentik_sdk::types::AgentEvent::Thinking(thinking.clone()));
                }
                ContentBlock::Text { text } if !text.is_empty() => {
                    self.send_event(agentik_sdk::types::AgentEvent::LlmResponse(text.clone()));
                }
                _ => {}
            }
        }

        self.token_budget.latest_usage = last_usage.input_tokens + last_usage.output_tokens;

        // Always remember the LLM response so the final text (if any) is preserved
        // in conversation history before we decide whether to terminate.
        self.memory.remember(response_message.clone())?;

        let toolcalls = self.extract_toolcalls(&response_message);

        // No tool calls in the response means the agent has finished its work.
        // This aligns with the LLM's trained prior ("produce final text = done"),
        // removing the retry-loop failure mode where the model never explicitly
        // calls `attempt_complete`. The lifecycle is flipped to IDLE so the
        // outer `start()` loop exits.
        if toolcalls.is_empty() {
            // self.lifecycle.set_idle();
            self.stop();
            return Ok(());
        }

        for tc in &toolcalls {
            self.send_event(agentik_sdk::types::AgentEvent::ToolCall {
                name: tc.name.clone(),
                input: tc.input.clone(),
            });
        }

        let tool_results = self
            .toolset
            .execute(
                &toolcalls,
                Some(self.internal_event_tx.clone()),
            )
            .await?;
        tracing::debug!(?tool_results, "tool execution results");

        for tr in &tool_results {
            // Background transitions are announced by the `Toolset` itself
            // (it owns `agent_event_tx` and observes the sync→async boundary
            // directly). Here we only surface results for tools that finished
            // synchronously; pending-task placeholders are skipped.
            // let is_placeholder = matches!(&tr.content, ToolResultContent::Text(t) if t.contains("is running in backend"));
            // if !is_placeholder {
            self.send_event(agentik_sdk::types::AgentEvent::ToolResult {
                ok: !tr.is_error.unwrap_or_default(),
                content: tr.text_content(),
            });
            // }
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

    /// lifecycle method
    fn stop(&mut self) {
        self.lifecycle.set_idle();
    }

    /// Poll the optional context provider for dynamic data.
    /// If it returns Some(text), inject as a user message.
    async fn poll_context_provider(&mut self) {
        if let Some(provider) = &self.context_provider {
            if let Some(text) = provider.poll().await {
                let _ = self.memory.remember(Message::user(text));
            }
        }
    }

    /// Return the tool whitelist for the active skill's current step, or
    /// `None` when no skill is attached (meaning all tools are allowed).
    async fn current_allowed_tools(&self) -> Option<Vec<String>> {
        let rt = self.skill_runtime.as_ref()?;
        let guard = rt.lock().await;
        Some(guard.allowed_tools_for_current_step())
    }

    /// Tool definitions the LLM is offered this turn: every registered
    /// tool, optionally narrowed to the active skill's current-step
    /// whitelist. Filtering lives here (not in `Toolset`) because it is
    /// orchestration policy, not a property of the tool registry itself.
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

    async fn build_context(&mut self) -> Result<Vec<Message>, AgentError> {
        use crate::prompt::context::Context;

        let mut builder =
            system_prompt_builder::SystemPromptBuilder::default().build_tooluse_guidance();

        if let Some(ref identity) = self.system_prompt_identity {
            builder = builder.with_identity(identity);
        }

        if let Some(ref extra) = self.system_prompt_section {
            builder = builder.with_extra_section(extra);
        }

        // Inject the active skill's current step / todo progress.
        if let Some(rt) = &self.skill_runtime {
            let section = rt.lock().await.current_prompt_section();
            if !section.is_empty() {
                builder = builder.with_extra_section(section);
            }
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
        context: Vec<Message>,
        allowed: Option<&[String]>,
    ) -> Result<Message, AgentError> {
        let span = span!(Level::TRACE, "API Request");
        let _enter = span.enter();

        let model = if let Some(name) = &self.current_model_name {
            self.model_pool
                .get_model_by_name(name)
                .unwrap_or_else(|_| self.model_pool.get_model_roundrobin().unwrap())
        } else {
            self.model_pool.get_model_roundrobin().unwrap()
        };

        // Accurate overflow detection using full message-list token estimation,
        // matching OpenCode's `compactIfNeeded()` logic.
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
            self.memory.compact(model.as_ref()).await?;
            // Rebuild context after compaction
            return Err(AgentError::CompactionRebuild);
        }

        let all_tools = self.visible_tools(allowed);

        let mut stream = model.request_stream(context, &all_tools).await?;

        while let Some(event) = stream.next().await {
            let stream_event = match event {
                Ok(e) => e,
                Err(e) => {
                    // poll_next already skips lagged events, but handle
                    // them defensively here as well (belt-and-suspenders).
                    match &e {
                        agentik_sdk::types::AnthropicError::StreamError(msg)
                            if msg.starts_with("Stream lagged:") =>
                        {
                            tracing::debug!("skipping lagged event: {e}");
                            continue;
                        }
                        _ => {
                            tracing::warn!("stream event error: {e}; breaking stream loop");
                            break;
                        }
                    }
                }
            };

            if let Some(agent_event) = AgentEvent::from_stream_event(&stream_event) {
                self.send_event(agent_event);
            }
        }
        tracing::info!("stream loop exited, awaiting final_message");
        // NB: do NOT emit `AgentEvent::Done` here. `Done` is a
        // lifecycle signal that the TUI uses to flip its `agent_running`
        // flag and re-enable the input field. Emitting it after every
        // LLM response — including the intermediate ones that are
        // followed by tool calls and another round-trip — caused the
        // TUI to think the agent had finished mid-iteration, which
        // collapsed the spinner into the "Enter to type" hint while
        // tool calls and the next streaming response were still in
        // flight. The lifecycle-based `Done` at the bottom of
        // `start()` is the single correct emission point.
        // (See also `AgentEvent::from_stream_event` for
        // `MessageStop`, which returns `None` for the same reason.)

        let response =
            tokio::time::timeout(std::time::Duration::from_secs(5), stream.final_message())
                .await
                .map_err(|e| AgentError::WorkflowFailed {
                    iteration: 0,
                    error: Box::new(AgentError::MissingConfig(format!(
                        "final_message() timed out: {e}"
                    ))),
                })??;

        tracing::debug!(?response, "LLM response");

        Ok(response)
    }

    /// Filter to extract ToolUse from LLM response message, Convert ContentBlock::ToolUse into
    /// ToolUse
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

/// Default token buffer before compaction triggers (matching OpenCode).
const COMPACTION_BUFFER_TOKENS: u64 = 20_000;

#[derive(Default)]
pub struct TokenBudget {
    append_tokens: u64,
    latest_usage: u64,
}
impl TokenBudget {
    /// Estimate token count for a single message.
    ///
    /// Uses the actual `input_tokens` from the API response if available,
    /// otherwise falls back to the chars/4 heuristic (matching OpenCode's
    /// `Token.estimate()`).
    pub fn count_token_est(&self, msg: &Message) -> u64 {
        if let Some(usage) = &msg.usage {
            return usage.input_tokens;
        }

        let content_str = serde_json::to_string(&msg.content)
            .expect("Convert message to JSON string failed during counting token budget");

        content_str.len() as u64 / 4
    }

    /// Estimate token count for an entire message list.
    ///
    /// This is the accurate version used for compaction decisions.
    /// It sums per-message estimates (preferring API-reported tokens when
    /// available, falling back to chars/4).
    pub fn estimate_messages_tokens(&self, messages: &[Message]) -> u64 {
        messages.iter().map(|m| self.count_token_est(m)).sum()
    }

    pub fn increment_new_msg(&mut self, msg: &Message) {
        self.append_tokens = self.count_token_est(msg);
    }

    pub fn estimate_total_token(&self, system_prompt_token: u64) -> u64 {
        self.append_tokens + self.latest_usage + system_prompt_token
    }

    /// Determine whether the conversation should be compacted.
    ///
    /// Uses the accurate full-message-list token estimate rather than the
    /// crude append-only heuristic. Mirrors OpenCode's `compactIfNeeded()`:
    /// triggers when total tokens >= context - max(output_tokens, buffer).
    pub fn should_compact(
        &self,
        messages: &[Message],
        context_length: u64,
        max_output_tokens: u64,
    ) -> bool {
        let total = self.estimate_messages_tokens(messages);
        let reserve = max_output_tokens.max(COMPACTION_BUFFER_TOKENS);
        let usable = context_length.saturating_sub(reserve);
        total >= usable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::dummy_model_info;
    use agentik_sdk::model::Model;
    use agentik_sdk::model::model_info::ModelInfo;
    use agentik_sdk::provider::client::MockApiClient;
    use agentik_sdk::types::AgentEvent;
    use agentik_sdk::types::messages::{ContentBlock, Message, Role};
    use agentik_sdk::types::shared::Usage;

    // ── Helpers ────────────────────────────────────────────────

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
        let mut model_pool = ModelPool::new();
        model_pool.add_model(Model::with_client(test_model_info(), mock_api));

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let mut agent = Agent::builder()
            .with_model_pool(Arc::new(model_pool))
            .with_config(AgentConfig {
                max_iterations: 5,
                max_retries: 0,
            })
            .build()
            .await
            .unwrap();

        agent.agent_event_tx = Some(tx);
        (agent, rx)
    }

    /// Collect all events from the receiver until channel closes or timeout.
    async fn collect_events(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    ) -> Vec<AgentEvent> {
        let mut events = vec![];
        while let Some(e) = rx.recv().await {
            events.push(e);
        }
        events
    }

    // ── Tests ─────────────────────────────────────────────────

    #[tokio::test]
    #[ignore] // slow: depends on network I/O / heavy mock setup
    async fn test_events_received_on_simple_text_response() {
        // TODO: configure MockApiClient.expect_request_stream() to return
        // MessageStream::from_events(vec![...], test_final_message("hello"))
        //
        let mock = MockApiClient::new();
        // mock.expect_request_stream()
        //     .returning(|_, _, _| { /* return mock stream */ });

        let (mut agent, mut rx) = build_test_agent(mock).await;

        tokio::spawn(async move {
            let _ = agent.run().await;
        });

        let _events = collect_events(&mut rx).await;

        // Verify event sequence contains:
        // LlmResponse("🤖 Agent started") → Requesting → TextDelta → ... → LlmResponse("hello") → Done
    }
}
