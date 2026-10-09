//! The session agent loop: turn lifecycle, internal-event application, the
//! core `agent_workflow`, the streaming LLM `request` with retries, and the
//! non-blocking `wait_task` background watchers.

use std::sync::Arc;
use std::time::Duration;

use agentik_sdk::model::sanitize::sanitize_messages;
use agentik_sdk::types::messages::{ContentBlock, Message};
use agentik_sdk::types::tools::ToolUse;
use agentik_sdk::types::{AgentEvent, AnthropicError};
use agentik_types::CompactTrigger;
use futures::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use tracing::{Level, span};
use uuid::Uuid;

use crate::agent::InternalEvent;
use crate::context::ContextProvider;
use crate::error::{AgentError, Result, Retryable};
use crate::message_ext::AgentMessageExt;
use crate::prompt::system_prompt_builder;
use crate::supervise::spawn_safe_drop;
use crate::tools::task_runtime::{TaskStatus, TaskStore};
use agentik_types::{AgentPlan, TurnExecutionStatus};

use super::Session;

const MAX_RETRY_BACKOFF_SECS: u64 = 30;

impl Session {
    // ── Turn lifecycle ─────────────────────────────────────

    /// Open a turn, reusing an open turn after a background-task wait.
    ///
    /// # Auguments
    /// * `delegation_id` - ID of parent agent if existed.
    pub(crate) fn begin_turn(&mut self, delegation_id: Option<Uuid>) {
        if self.active_turn_id.is_none() {
            self.active_turn_id = Some(Uuid::new_v4());
            self.active_delegation_id = delegation_id;
            self.telemetry.turn_count += 1;
            self.turn_baseline = Some(self.telemetry);
            self.turn_timing = super::TurnTiming::default();
        } else if delegation_id.is_some() {
            self.active_delegation_id = delegation_id;
        }
        self.turn_timing.start();

        self.shared.send_event(AgentEvent::TurnStarted {
            turn_id: self.active_turn_id.expect("turn id set above"),
            session_id: self.id,
            delegation_id: self.active_delegation_id,
        });
    }

    /// Close the current turn and emit its terminal status.
    pub(crate) fn finish_turn(&mut self, status: TurnExecutionStatus) {
        let Some(turn_id) = self.active_turn_id.take() else {
            return;
        };
        let delegation_id = self.active_delegation_id.take();
        let telemetry = self.finish_telemetry();
        self.shared.send_event(AgentEvent::TurnCompleted {
            turn_id,
            session_id: self.id,
            delegation_id,
            status,
            telemetry,
        });
    }

    // ── Session loop ──────────────────────────────────────

    /// Run one "session": a sequence of LLM round-trips until the agent
    /// goes idle, hits an error, or is cancelled.
    pub(crate) async fn run_session(
        &mut self,
        internal_event_tx: &UnboundedSender<InternalEvent>,
        rx: &mut UnboundedReceiver<InternalEvent>,
        delegation_id: Option<Uuid>,
    ) {
        self.begin_turn(delegation_id);
        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Requesting);

        // Ensure the toolset is wired to the current cancel token so that
        // Ctrl+C interrupts running tools. ResetCancelToken updates this
        // on subsequent turns, but the first turn needs it set here.
        self.toolset.set_cancel_token(self.cancel_token.clone());

        // Ensure WAL session is open.
        if self.persist_tx.is_none() {
            if let Some(tx) = self.shared.persist_tx.get() {
                self.persist_tx = Some(tx.clone());
            }
        }
        if let Some(storage) = &self.shared.storage {
            let _ = storage.start_session(self.shared.id, self.id).await;
            let _ = storage.touch_agent(self.shared.id).await;
        }

        let mut was_cancelled = false;

        loop {
            // Check for cancellation before each iteration.
            if self.cancel_token.is_cancelled() {
                was_cancelled = true;
                break;
            }

            // Drain queued events (retry feedback, bg-task notices, etc.)
            let mut retry_feedback: Option<String> = None;
            while let Ok(event) = rx.try_recv() {
                if !self.apply_internal_event(event).await {
                    // Terminal control event — stop the loop.
                    break;
                }
            }

            // Run one iteration of the agent workflow.
            let session_done = match self
                .agent_workflow(internal_event_tx, retry_feedback.take())
                .await
            {
                Ok(()) => {
                    // After agent_workflow, if the lifecycle is still
                    // "running" it means tool calls were executed and the
                    // agent needs another API round. Only stop when the
                    // lifecycle has transitioned away from running (e.g.
                    // Idle via self.stop(), Error, or Waiting).
                    if !self.lifecycle.is_running() {
                        // Don't emit `Done` when waiting for background
                        // tasks — the turn isn't complete, just paused.
                        // The watcher will inject a message to resume.
                        if *self.lifecycle.status() != agentik_types::AgentLifecycleStatus::Waiting
                        {
                            self.finish_turn(TurnExecutionStatus::Completed);
                            self.shared.send_event(AgentEvent::Done);
                        }
                        true
                    } else {
                        false
                    }
                }
                Err(e) => {
                    if self.cancel_token.is_cancelled() {
                        was_cancelled = true;
                        break;
                    }
                    tracing::error!(
                        error = %e,
                        error_chain = ?e,
                        "workflow failed"
                    );
                    self.shared.send_event(AgentEvent::Error(format!("{e}")));
                    self.persist_snapshot().await;
                    self.set_lifecycle(agentik_types::AgentLifecycleStatus::Error);
                    self.finish_turn(TurnExecutionStatus::Failed);
                    true
                }
            };

            if session_done {
                break;
            }

            // Drain control/notification events that arrived during this iteration.
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
                        | InternalEvent::Shutdown
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
                self.pause_telemetry();
                self.stop();
                break;
            }
            if terminal {
                break;
            }
        }

        // Post-loop cleanup.
        if was_cancelled {
            self.set_lifecycle(agentik_types::AgentLifecycleStatus::Cancelled);
            self.persist_snapshot().await;
            let _ = self.remember(Message::user(
                "[interrupted] The user interrupted the previous turn on purpose. \
                 Any tool calls may have been partially executed. \
                 Background tasks may still be running.",
            ));
            self.shared.send_event(AgentEvent::TurnAborted);
            self.finish_turn(TurnExecutionStatus::Interrupted);
        } else if self.lifecycle.is_running() {
            self.set_lifecycle(agentik_types::AgentLifecycleStatus::Idle);
        }
    }

    // ── Apply internal events ─────────────────────────────

    /// Apply a single [`InternalEvent`] against session state.
    ///
    /// Returns `true` for events that represent new work; `false` for
    /// terminal control signals.
    pub(crate) async fn apply_internal_event(&mut self, event: InternalEvent) -> bool {
        match event {
            InternalEvent::MessageInject {
                content,
                from_user,
                delegation_id,
            } => {
                if delegation_id.is_some() {
                    self.active_delegation_id = delegation_id;
                }
                let _ = self.inject_message(content, from_user);
                true
            }
            InternalEvent::Compact => {
                self.do_manual_compact().await;
                // Compact doesn't start a new LLM turn by itself — the
                // caller (TUI / command) can inject a follow-up message
                // if it wants the agent to resume work. Return true so the
                // agent's run() loop continues listening for events.
                true
            }
            InternalEvent::BgTaskComplete { id: _, seq } => {
                // If a wait_task watcher is active for this task, suppress the
                // generic "has completed" notification — the watcher will
                // inject a more detailed message with the actual result.
                if self.active_wait_watchers.contains_key(&seq) {
                    tracing::debug!(
                        seq,
                        "suppressing BgTaskComplete notification — wait_watcher is active"
                    );
                    // Still emit the UI event so the task panel updates.
                    if let Some((_, ok)) = self.toolset.task_brief(seq).await {
                        self.shared
                            .send_event(AgentEvent::ToolBackgroundComplete { seq, ok });
                    }
                    return true;
                }
                if let Some((name, ok)) = self.toolset.task_brief(seq).await {
                    self.shared
                        .send_event(AgentEvent::ToolBackgroundComplete { seq, ok });
                    let note = if ok {
                        format!(
                            "Background task '{name}' (#{seq}) has completed. \
                             Use `view_task_results` with task={seq} to view the output."
                        )
                    } else {
                        format!(
                            "Background task '{name}' (#{seq}) has failed. \
                             Use `view_task_results` with task={seq} to view the error details."
                        )
                    };
                    let _ = self.remember(Message::user(note));
                }
                true
            }
            InternalEvent::Shutdown => {
                self.finish_turn(TurnExecutionStatus::Interrupted);
                self.set_lifecycle(agentik_types::AgentLifecycleStatus::Idle);
                false
            }
            InternalEvent::Done => {
                self.finish_turn(TurnExecutionStatus::Completed);
                self.stop();
                false
            }
            InternalEvent::ResetCancelToken(token) => {
                // If we were Waiting, the cancel interrupted the wait —
                // transition to Cancelled so the agent is ready for new work.
                // Background watchers have a child token tied to the old
                // (now-cancelled) token, so they exit silently.
                self.active_wait_watchers.clear();
                if *self.lifecycle.status() == agentik_types::AgentLifecycleStatus::Waiting {
                    self.set_lifecycle(agentik_types::AgentLifecycleStatus::Cancelled);
                    self.shared.send_event(AgentEvent::TurnAborted);
                }
                self.finish_turn(TurnExecutionStatus::Interrupted);
                self.cancel_token = token.clone();
                self.toolset.cancel_all_tasks().await;
                // Wire the fresh cancel token into the toolset so that
                // Ctrl+C also interrupts running tool tasks.
                self.toolset.set_cancel_token(token);
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

    // ── Core agent workflow ───────────────────────────────

    pub(super) async fn agent_workflow(
        &mut self,
        internal_event_tx: &UnboundedSender<InternalEvent>,
        retry_feedback: Option<String>,
    ) -> Result<()> {
        if let Some(feedback) = retry_feedback {
            self.inject_message(vec![ContentBlock::Text { text: feedback }], false)
                .unwrap();
        }

        self.poll_context_provider().await;

        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Requesting);
        self.shared.send_event(AgentEvent::Requesting);
        let response_message = self.request_with_retries().await?;

        let last_usage = response_message.usage.clone().unwrap_or_default();
        self.telemetry.record_usage(&last_usage);
        self.last_active = chrono::Utc::now().timestamp_millis();
        self.persist_telemetry();

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

        self.token_budget.record_api_usage(&last_usage);

        self.remember(response_message.clone())?;
        self.token_budget.add_pending_message(&response_message);

        let toolcalls = self.extract_toolcalls(&response_message);

        if toolcalls.is_empty() {
            self.stop();
            return Ok(());
        }

        self.telemetry
            .record_tool_use(toolcalls.len().try_into().unwrap_or(u64::MAX));
        self.last_active = chrono::Utc::now().timestamp_millis();
        self.persist_telemetry();

        for tc in &toolcalls {
            self.shared.send_event(AgentEvent::ToolCall {
                name: tc.name.clone(),
                input: tc.input.clone(),
            });
        }

        let has_wait_task = toolcalls.iter().any(|tc| tc.name == "wait_task");
        if has_wait_task {
            self.set_lifecycle(agentik_types::AgentLifecycleStatus::Waiting);
        } else {
            self.set_lifecycle(agentik_types::AgentLifecycleStatus::ToolRunning);
        }

        let tool_results = self
            .toolset
            .execute(&toolcalls, Some(internal_event_tx.clone()))
            .await?;
        let failed_tool_results = tool_results
            .iter()
            .filter(|result| result.is_error.unwrap_or(false))
            .count()
            .try_into()
            .unwrap_or(u64::MAX);
        self.telemetry.record_tool_results(
            tool_results.len().try_into().unwrap_or(u64::MAX),
            failed_tool_results,
        );
        self.last_active = chrono::Utc::now().timestamp_millis();
        self.persist_telemetry();

        // ── Non-blocking wait_task: detect "waiting" results ──
        //
        // The `wait_task` tool now returns immediately with `status: "waiting"`
        // when the target task is still running (instead of blocking). Here we
        // detect those results, spawn background watchers, and keep the
        // lifecycle as `Waiting` so the session loop exits — just like going
        // `Idle`. When a watcher fires, it injects a message that re-enters
        // `run_session`.
        let mut registered_waits = false;
        for tr in &tool_results {
            // Match the result back to its toolcall to see if it was wait_task.
            let is_wait_task = toolcalls
                .iter()
                .find(|tc| tc.id == tr.tool_use_id)
                .is_some_and(|tc| tc.name == "wait_task");
            if !is_wait_task {
                continue;
            }
            // Check the result content for the "waiting" sentinel.
            let content = tr.text_content();
            if content.contains("\"status\":\"waiting\"")
                || content.contains("\"status\": \"waiting\"")
            {
                // Extract task number and timeout from the wait_task input.
                let tc = toolcalls
                    .iter()
                    .find(|tc| tc.id == tr.tool_use_id)
                    .expect("matched above");
                let task_seq = tc.input.get("task").and_then(|v| v.as_u64()).unwrap_or(0);
                let timeout_secs = tc
                    .input
                    .get("timeout_seconds")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(600)
                    .max(300); // floor: 别让模型传小值反复空醒烧 turn

                // Cancel any existing watcher for this task before spawning a
                // new one. This prevents duplicate message injection when the
                // LLM calls wait_task on the same still-running task multiple
                // times (either across turns or as parallel tool calls).
                if let Some(old) = self.active_wait_watchers.remove(&task_seq) {
                    old.cancel();
                    tracing::debug!(
                        task_seq,
                        "cancelled previous watcher before spawning new one"
                    );
                }

                // Spawn a background watcher that will inject a message when
                // the task completes or the timeout expires. A child cancel
                // token ensures the watcher exits silently if the user
                // interrupts (Ctrl+C) while Waiting.
                let tasks = self.shared.tasks.clone();
                let tx = internal_event_tx.clone();
                let cancel = self.cancel_token.child_token();
                self.active_wait_watchers.insert(task_seq, cancel.clone());
                crate::supervise::spawn_safe_drop(
                    "wait_watcher",
                    Self::wait_watcher(tasks, task_seq, timeout_secs, tx, cancel),
                );
                registered_waits = true;
                tracing::info!(
                    task_seq,
                    timeout_secs,
                    "registered background watcher for wait_task"
                );
            }
        }

        if registered_waits {
            self.pause_telemetry();
            // Keep lifecycle as `Waiting` — the session loop will exit.
            // The watcher(s) will inject messages to re-enter run_session.
            // Emit tool results so the UI shows the "waiting" status.
            for tr in &tool_results {
                self.shared.send_event(AgentEvent::ToolResult {
                    ok: !tr.is_error.unwrap_or_default(),
                    content: tr.text_content(),
                });
            }
            for tr in &tool_results {
                let msg = Message::tool_result(
                    tr.tool_use_id.clone(),
                    tr.text_content(),
                    tr.is_error.unwrap_or_default(),
                );
                self.remember(msg.clone())?;
                self.token_budget.add_pending_message(&msg);
            }
            // Images returned by tools are flattened away by the text-only
            // tool_result messages above; re-attach them for vision models.
            self.remember_tool_result_images(&tool_results, &toolcalls)?;
            // Skip compaction — there's no immediate next LLM call.
            return Ok(());
        }

        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Requesting);

        for tr in &tool_results {
            self.shared.send_event(AgentEvent::ToolResult {
                ok: !tr.is_error.unwrap_or_default(),
                content: tr.text_content(),
            });
        }

        for tr in &tool_results {
            let msg = Message::tool_result(
                tr.tool_use_id.clone(),
                tr.text_content(),
                tr.is_error.unwrap_or_default(),
            );
            self.remember(msg.clone())?;
            self.token_budget.add_pending_message(&msg);
        }
        // Images returned by tools are flattened away by the text-only
        // tool_result messages above; re-attach them for vision models.
        self.remember_tool_result_images(&tool_results, &toolcalls)?;

        // ── Mid-turn compaction check (ported from Codex) ──
        //
        // After executing tools, the model will need a follow-up call to
        // process the results. If the context has grown past the auto-compact
        // threshold, compact *now* to prevent an overflow on the next request.
        if !toolcalls.is_empty() {
            let context_length = self
                .shared
                .model
                .load()
                .as_ref()
                .map(|m| m.model_info.context_length)
                .unwrap_or(0);
            let conversation_msgs = self.render_context()?;
            if self
                .token_budget
                .should_compact(&conversation_msgs, context_length)
            {
                let model = self.shared.model.load_full().ok_or_else(|| {
                    AgentError::MissingConfig("no active model configured".into())
                })?;
                let used_pct = self
                    .token_budget
                    .context_fill_pct(&conversation_msgs, context_length);
                tracing::debug!(
                    context_length,
                    current_total = self.token_budget.current_total(),
                    "mid-turn context pressure detected, compacting before next iteration"
                );
                self.compact(
                    model.as_ref(),
                    CompactTrigger::MidTurn { used_pct },
                    agentik_types::AgentLifecycleStatus::Requesting,
                )
                .await?;
            }
        }

        Ok(())
    }

    /// Retry retryable API failures without replaying tool side effects.
    ///
    /// The retry boundary is intentionally inside the workflow's model-request
    /// phase. Retrying the whole workflow after a later failure could execute
    /// tools twice, while retrying here only repeats the LLM request.
    /// Each attempt rebuilds fresh state inside `request`.
    async fn request_with_retries(&mut self) -> Result<Message> {
        let max_retries: u32 = self
            .shared
            .config
            .max_retries
            .try_into()
            .unwrap_or(u32::MAX);
        let mut attempts_used = 0u32;

        loop {
            match self.request().await {
                Ok(response) => return Ok(response),
                Err(e) if attempts_used < max_retries && is_retryable_api_request(&e) => {
                    attempts_used += 1;
                    let backoff_secs = 1u64
                        .checked_shl((attempts_used - 1).min(u32::BITS - 1))
                        .unwrap_or(u64::MAX)
                        .min(MAX_RETRY_BACKOFF_SECS);

                    tracing::warn!(
                        error = %e,
                        attempt = attempts_used,
                        max_retries,
                        backoff_secs,
                        "retrying retryable API request error"
                    );
                    self.shared.send_event(AgentEvent::RetryableError {
                        message: format!("{e}"),
                        attempt: attempts_used,
                        max_retries,
                    });

                    tokio::select! {
                        biased;
                        _ = self.cancel_token.cancelled() => {
                            return Err(AgentError::Cancelled);
                        }
                        _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
                    }

                    self.set_lifecycle(agentik_types::AgentLifecycleStatus::Requesting);
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn request(&mut self) -> Result<Message> {
        let span = span!(Level::TRACE, "API Request");
        let _enter = span.enter();

        let model = self
            .shared
            .model
            .load_full()
            .ok_or_else(|| AgentError::MissingConfig("no active model configured".into()))?;

        let conversation_msgs = self.render_context()?;
        if self
            .token_budget
            .should_compact(&conversation_msgs, model.model_info.context_length)
        {
            let used_pct = self
                .token_budget
                .context_fill_pct(&conversation_msgs, model.model_info.context_length);
            tracing::debug!(
                context_length = model.model_info.context_length,
                current_total = self.token_budget.current_total(),
                "pre-request context pressure detected (≥90% of context window), compacting"
            );
            let compacted = self
                .compact(
                    model.as_ref(),
                    CompactTrigger::PreRequest { used_pct },
                    agentik_types::AgentLifecycleStatus::Requesting,
                )
                .await?;
            if !compacted {
                tracing::warn!(
                    "context pressure detected but nothing to compact; \
                     proceeding with request (current segment may overflow)"
                );
            }
        }

        let all_tools = self.toolset.tools();

        // Repair any Anthropic-invariant violations before sending and
        // persist the repaired shape so future turns don't re-patch.
        self.sanitize_and_persist();
        // sanitize_and_persist may have modified self.messages (dedup,
        // coalesce, adjacency repair). Rebuild context from the
        // repaired messages so the API receives the clean shape.
        let built_context = self.build_context().await?;

        // Final defensive sanitize on the actual context (without
        // system prompt — it's stored separately now, so there's no
        // same-role coalescing risk).
        let context = sanitize_messages(built_context);

        // Race the initial HTTP request against cancellation so that
        // Ctrl+C interrupts even before the first stream event arrives.
        // System prompt is passed via params.system (top-level field),
        // NOT mixed into messages — avoids same-role coalescing.
        let system = self.pending_system_prompt.take();
        let mut stream = tokio::select! {
            r = model.request_stream_with_system(context, &all_tools, system) => r?,
            _ = self.cancel_token.cancelled() => {
                tracing::info!("LLM request cancelled before stream started");
                return Err(crate::error::AgentError::Cancelled);
            }
        };

        loop {
            // Race the next stream event against cancellation so that
            // Ctrl+C interrupts an in-flight LLM stream immediately.
            let event = tokio::select! {
                ev = stream.next() => match ev {
                    Some(ev) => ev,
                    None => break,
                },
                _ = self.cancel_token.cancelled() => {
                    tracing::info!("LLM stream cancelled by user");
                    stream.abort();
                    return Err(crate::error::AgentError::Cancelled);
                }
            };

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
                if matches!(
                    agent_event,
                    AgentEvent::TextDelta(_) | AgentEvent::ThinkingDelta(_)
                ) {
                    self.set_lifecycle(agentik_types::AgentLifecycleStatus::Streaming);
                }
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

    // ── Wait-task watcher ─────────────────────────────────

    /// Background watcher for a non-blocking `wait_task`.
    ///
    /// Spawns as a fire-and-forget task when the agent calls `wait_task` on a
    /// still-running background task. It monitors the task's status via the
    /// watch channel, racing completion against a timeout. When either fires,
    /// it injects a [`InternalEvent::MessageInject`] to wake the agent.
    ///
    /// The agent's session loop has already exited (lifecycle = `Waiting`),
    /// so the injected message re-enters `run_session` and the LLM receives
    /// the result as a new user message.
    async fn wait_watcher(
        tasks: Arc<tokio::sync::RwLock<TaskStore>>,
        task_seq: u64,
        timeout_secs: u64,
        tx: tokio::sync::mpsc::UnboundedSender<InternalEvent>,
        cancel: CancellationToken,
    ) {
        use std::time::Duration;

        // Clone the status watch receiver while holding the read lock briefly.
        let status_rx = {
            let tasks = tasks.read().await;
            match tasks.find_by_seq(task_seq) {
                Some(task) if matches!(task.status(), TaskStatus::Running) => {
                    Some(task.status_receiver_clone())
                }
                _ => None, // Already completed or not found — handle below.
            }
        };

        // If the task already finished (race), inject immediately.
        let Some(mut status_rx) = status_rx else {
            // Read the result if available.
            let message = {
                let tasks = tasks.read().await;
                match tasks.find_by_seq(task_seq) {
                    Some(task) => match task.tool_result() {
                        Some(result) => {
                            let is_error = result.is_error.unwrap_or(false);
                            let content = result.text_content();
                            let label = if is_error { "error" } else { "completed" };
                            format!(
                                "Background task '{}' (#{task_seq}) has {label}.\nResult:\n{content}",
                                task.name()
                            )
                        }
                        None => format!(
                            "Background task '{}' (#{task_seq}) is no longer running.",
                            task.name()
                        ),
                    },
                    None => format!("Background task #{task_seq} no longer exists."),
                }
            };
            let _ = tx.send(InternalEvent::MessageInject {
                content: vec![ContentBlock::Text { text: message }],
                from_user: false,
                delegation_id: None,
            });
            return;
        };

        // Race task completion against timeout and user cancellation.
        let completed = tokio::select! {
            result = status_rx.changed() => result.is_ok(),
            _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => false,
            _ = cancel.cancelled() => {
                tracing::info!(task_seq, "wait_watcher cancelled by user");
                return; // Silent exit — don't inject a wakeup message.
            }
        };

        let message = if completed {
            // Task status changed — read the actual result.
            let tasks = tasks.read().await;
            match tasks.find_by_seq(task_seq) {
                Some(task) => match task.status() {
                    TaskStatus::Done(_) => match task.tool_result() {
                        Some(result) => {
                            let content = result.text_content();
                            format!(
                                "Background task '{}' (#{task_seq}) has completed successfully.\nResult:\n{content}",
                                task.name()
                            )
                        }
                        None => format!(
                            "Background task '{}' (#{task_seq}) has completed.",
                            task.name()
                        ),
                    },
                    TaskStatus::Failed(ref e) => format!(
                        "Background task '{}' (#{task_seq}) has failed:\n{e}",
                        task.name()
                    ),
                    TaskStatus::Running => format!(
                        "Background task '{}' (#{task_seq}) status unclear, still running.",
                        task.name()
                    ),
                },
                None => format!("Background task #{task_seq} no longer exists."),
            }
        } else {
            // Timeout — task is still running.
            let tasks = tasks.read().await;
            let name = tasks
                .find_by_seq(task_seq)
                .map(|t| t.name().to_string())
                .unwrap_or_else(|| format!("#{task_seq}"));
            format!(
                "Background task '{name}' (#{task_seq}) did not complete within {timeout_secs} seconds. \
                 It is still running. Check its status with `view_task_status`, or `wait_task` again \
                 with a LARGER timeout_seconds (e.g. 1800) — you are woken the moment it finishes, \
                 so a large timeout costs nothing."
            )
        };

        let _ = tx.send(InternalEvent::MessageInject {
            content: vec![ContentBlock::Text { text: message }],
            from_user: false,
            delegation_id: None,
        });
    }

    // ── Workflow helpers ──────────────────────────────────

    pub(crate) fn stop(&mut self) {
        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Idle);
    }

    async fn poll_context_provider(&mut self) {
        if let Some(provider) = &self.shared.context_provider {
            if let Some(text) = provider.poll().await {
                let _ = self.remember(Message::user(text));
            }
        }
    }

    async fn build_context(&mut self) -> Result<Vec<Message>> {
        // Skill-library guidance is static behavioral text (how to
        // consume and feed the skill evolution tools), so it lives in
        // the system prompt; the volatile skill *index* stays out
        // (see the notice note below for why).
        let mut builder = system_prompt_builder::SystemPromptBuilder::default()
            .build_skill_guidance()
            .build_tooluse_guidance();

        let identity = self
            .shared
            .system_prompt_identity
            .as_deref()
            .unwrap_or("You are a helpful assistant.");
        builder = builder.with_identity(format!(
            "{identity}\n\n\
             Your agent path is `{path}`. Your short name is **{name}**. \
             You can see and interact only with your direct children \
             (spawned under your path) and your direct parent. \
             When spawning child agents, their path is derived from yours. \
             Discover visible agents with `list_agents` or `route_task`, \
             delegate to your children with `delegate_to`, and never \
             delegate tasks to yourself.",
            path = self.shared.path.as_str(),
            name = self.shared.name(),
        ));

        if let Some(ref extra) = self.shared.system_prompt_section {
            builder = builder.with_extra_section(extra);
        }

        // Inject cross-session memory. with_extra_section appends, so the
        // memory summary supplements the agent's profile section above
        // instead of replacing it.
        if let Some(memory) = &self.shared.memory {
            if let Some(section) = memory.prompt_section().await {
                builder = builder.with_extra_section(section);
            }
        }

        // 注:plan 状态不再进 system——它随任务推进高频变化,每次变化
        // 都会重写 instructions(请求体最头部),使 prompt-cache 的最长
        // 公共前缀归零,并轮换 ChatGPT wire 的 prompt_cache_key。改为
        // 变更时向消息流尾部注入通知(inject_state_notices),system 在
        // 会话内保持逐字节稳定。
        let system_prompt = builder.parse();

        self.inject_state_notices().await;

        let context_messages = self.render_context()?.to_vec();

        // Store system prompt on the session so request() can pass it
        // via params.system instead of mixing it into messages.
        self.pending_system_prompt = Some(system_prompt);

        Ok(context_messages)
    }

    /// 将 plan 状态以**消息流尾部通知**同步给模型:仅当状态与
    /// 历史中最近一条同类通知不一致时追加一条 user 消息(前缀
    /// [`PLAN_NOTICE_PREFIX`]);状态未变则不
    /// 注入——幂等,重试与 snapshot 恢复安全。
    ///
    /// 通知经 `remember` 落库;压缩把旧通知摘要掉后,下次构建在历史中
    /// 找不到匹配通知,即按当前状态重发(自愈)。
    async fn inject_state_notices(&mut self) {
        // plan
        let plan = self.shared.plan_snapshot();
        let plan_section = (!plan.is_empty()).then(|| render_plan_prompt_section(&plan));
        if !notice_matches(&self.messages, PLAN_NOTICE_PREFIX, plan_section.as_deref()) {
            let _ = self.remember(Message::user(notice_text(
                PLAN_NOTICE_PREFIX,
                plan_section.as_deref(),
            )));
        }
    }
}

fn is_retryable_api_request(error: &AgentError) -> bool {
    matches!(error, AgentError::ApiRequestError(e) if e.is_retryable())
}

/// Render the current plan as a system-prompt section.
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

// ── Tail-injected state notices (plan) ─────────────────────────────
//
// 这些状态随任务推进高频变化,放进 system 会打掉 prompt-cache 前缀
// (见 build_context 注释)。改为:变化时在消息流尾部追加一条带前缀
// 标记的 user 通知;未变化时不追加。

/// 尾部通知:plan 状态通知的前缀(同时也是历史扫描标记)。
const PLAN_NOTICE_PREFIX: &str = "[plan-status]\n";
/// 通知正文里表示"状态已清空"的哨兵;历史中出现它等价于当前无状态。
const NOTICE_CLEARED_BODY: &str = "(cleared — no longer active)";

/// 历史中最近一条该前缀通知所反映的状态正文;外层 `None` = 从未注入。
/// `(cleared)` 哨兵归一化为内层 `None`,使"注入过清空通知"与"当前无
/// 状态"可以判等。
fn last_notice_state<'a>(messages: &'a [Message], prefix: &str) -> Option<Option<&'a str>> {
    messages
        .iter()
        .rev()
        .flat_map(|m| m.content.iter())
        .filter_map(|c| match c {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .find_map(|t| t.strip_prefix(prefix))
        .map(|body| (body != NOTICE_CLEARED_BODY).then_some(body))
}

/// 当前状态(`current`,`None` = 未激活)是否已被历史中最近一条通知
/// 如实反映。从未注入且未激活视为一致(不注入)。
fn notice_matches(messages: &[Message], prefix: &str, current: Option<&str>) -> bool {
    match (last_notice_state(messages, prefix), current) {
        (None, None) | (Some(None), None) => true,
        (Some(Some(last)), Some(current)) => last == current,
        _ => false,
    }
}

/// 组装一条通知消息的全文。
fn notice_text(prefix: &str, current: Option<&str>) -> String {
    match current {
        Some(body) => format!("{prefix}{body}"),
        None => format!("{prefix}{NOTICE_CLEARED_BODY}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::AgentShared;
    use super::*;
    use crate::agent::AgentConfig;

    // ── System prompt per-turn hashing (cache prefix stability) ─────

    /// system 稳定性 + plan 尾部通知注入实证:在模型调用边界
    /// (MockApiClient)捕获每次实际传出的 `system` 与消息列表,验证:
    /// 1. system 在**整个会话**内逐字节稳定(plan 状态变化不再
    ///    重写 instructions——prompt-cache 前缀的关键不变量);
    /// 2. `update_plan` 执行后,plan 状态以**消息流尾部通知**出现在
    ///    同一回合内的下一次请求里(携带渲染后的 plan);
    /// 3. 状态未变的后续回合不重复注入(历史中恰好一条通知)。
    #[tokio::test]
    async fn plan_state_injects_as_tail_notice_and_keeps_system_stable() {
        use crate::testing::dummy_model_info;
        use agentik_sdk::model::Model;
        use agentik_sdk::provider::client::MockApiClient;
        use agentik_sdk::streaming::MessageStream;
        use std::hash::{Hash, Hasher};

        let captured: Arc<std::sync::Mutex<Vec<(Vec<Message>, Option<String>)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let responses = [
            Message::assistant_text("hi there"),
            Message::assistant_tool_use(
                "call_plan_1",
                "update_plan",
                serde_json::json!({
                    "plan": [
                        {"step": "Investigate cache prefix", "status": "in_progress"},
                        {"step": "Write fix", "status": "pending"}
                    ]
                }),
            ),
            Message::assistant_text("plan saved"),
            Message::assistant_text("done"),
        ];
        let mut mock = MockApiClient::new();
        for resp in responses {
            let cap = Arc::clone(&captured);
            mock.expect_request_stream_with_system().times(1).returning(
                move |messages, _, _, system| {
                    cap.lock().unwrap().push((messages, system));
                    Ok(MessageStream::from_events(Vec::new(), resp.clone()))
                },
            );
        }
        let model = Model::with_client(dummy_model_info("sys-hash"), mock);

        // 与 AgentBuilder 相同的方式注册 update_plan(PlanHandle 指向
        // shared.plan 的同一个 ArcSwap)。
        let plan_state = Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
            agentik_types::AgentPlan::new(),
        )));
        let mut registry = crate::tools::ToolRegistry::new();
        registry
            .register_all(crate::tools::plan_registrations(
                crate::tools::builtins::PlanHandle::new(
                    Arc::clone(&plan_state),
                    Uuid::new_v4(),
                    None,
                    None,
                ),
            ))
            .unwrap();
        let shared = Arc::new(AgentShared {
            id: Uuid::new_v4(),
            path: agentik_types::AgentPath::root(),
            config_json: serde_json::json!({}),
            model: Arc::new(arc_swap::ArcSwapOption::from_pointee(Some(model))),
            config: AgentConfig::default(),
            storage: None,
            context_provider: None,
            system_prompt_section: None,
            system_prompt_identity: None,
            memory: None,
            tool_registry: Arc::new(registry),
            tasks: Arc::new(tokio::sync::RwLock::new(
                crate::tools::task_runtime::TaskStore::new(),
            )),
            event_tx: arc_swap::ArcSwapOption::empty(),
            persist_tx: std::sync::OnceLock::new(),
            plan: plan_state,
        });

        let (internal_tx, _internal_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut session = Session::new_for_tests(shared, agentik_types::AgentPath::root());

        session.remember(Message::user("hello")).unwrap();
        session.agent_workflow(&internal_tx, None).await.unwrap();
        session.remember(Message::user("now make a plan")).unwrap();
        session.agent_workflow(&internal_tx, None).await.unwrap();
        session.agent_workflow(&internal_tx, None).await.unwrap();
        session.remember(Message::user("thanks")).unwrap();
        session.agent_workflow(&internal_tx, None).await.unwrap();

        let calls = std::mem::take(&mut *captured.lock().unwrap());
        assert_eq!(calls.len(), 4, "exactly 4 model calls expected");

        let hash_of = |s: &Option<String>| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.as_deref().unwrap_or("<none>").hash(&mut h);
            format!("{:016x}", h.finish())
        };
        let notice_count = |messages: &[Message]| {
            messages
                .iter()
                .flat_map(|m| m.text())
                .filter(|t| t.starts_with(PLAN_NOTICE_PREFIX))
                .count()
        };
        for (i, (messages, system)) in calls.iter().enumerate() {
            println!(
                "call{} system_hash={} system_len={:4} plan_notices={}",
                i + 1,
                hash_of(system),
                system.as_deref().map(str::len).unwrap_or(0),
                notice_count(messages),
            );
        }

        // 1. system 全程逐字节稳定,且 plan 不再进 system。
        assert!(
            calls.iter().all(|(_, s)| s == &calls[0].1),
            "system must stay byte-identical across the whole conversation"
        );
        assert!(
            !calls.iter().any(|(_, s)| s
                .as_deref()
                .is_some_and(|s| s.contains("## Current plan status"))),
            "plan section must not leak into system"
        );

        // 2. update_plan 执行后的下一次请求以尾部通知携带 plan。
        assert_eq!(notice_count(&calls[0].0), 0, "cold call has no notice");
        assert_eq!(notice_count(&calls[1].0), 0, "pre-tool call has no notice");
        assert_eq!(
            notice_count(&calls[2].0),
            1,
            "post-update_plan continuation must carry exactly one plan notice"
        );
        assert!(
            calls[2].0.iter().any(|m| {
                m.text()
                    .iter()
                    .any(|t| t.contains("## Current plan status"))
            }),
            "the notice must carry the rendered plan"
        );

        // 3. 状态未变 → 不重复注入。
        assert_eq!(
            notice_count(&calls[3].0),
            1,
            "unchanged plan must not trigger a second notice"
        );
    }

    /// Regression: the memory summary section must *supplement* the agent's
    /// configured `system_prompt_section`, not replace it. The builder's
    /// `with_extra_section` used to overwrite, so any agent with a memory
    /// summary silently lost its profile prompt.
    #[tokio::test]
    async fn memory_summary_appends_to_custom_system_section() {
        use crate::memory::{MEMORY_SCOPE_ID, MemoryBackend, MemoryConfig, MemoryStore};
        use crate::testing::dummy_model_info;
        use agentik_sdk::model::Model;
        use agentik_sdk::provider::client::MockApiClient;
        use agentik_sdk::streaming::MessageStream;

        let captured: Arc<std::sync::Mutex<Vec<Option<String>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut mock = MockApiClient::new();
        let cap = Arc::clone(&captured);
        mock.expect_request_stream_with_system()
            .times(1)
            .returning(move |_, _, _, system| {
                cap.lock().unwrap().push(system);
                Ok(MessageStream::from_events(
                    Vec::new(),
                    Message::assistant_text("ok"),
                ))
            });
        let model = Model::with_client(dummy_model_info("mem-section"), mock);

        // Seed a memory summary the way phase 2 consolidation would.
        let store: Arc<dyn MemoryStore> =
            Arc::new(crate::TursoAgentStorage::open_in_memory().await.unwrap());
        store
            .complete_phase2(
                MEMORY_SCOPE_ID,
                "test-source",
                Vec::new(),
                "v1\n\nPrefer targeted tests.",
                Vec::new(),
                Vec::new(),
            )
            .await
            .unwrap();
        let memory = Arc::new(MemoryBackend::new(MemoryConfig::new(), store, None));

        let registry = crate::tools::ToolRegistry::new();
        let shared = Arc::new(AgentShared {
            id: Uuid::new_v4(),
            path: agentik_types::AgentPath::root(),
            config_json: serde_json::json!({}),
            model: Arc::new(arc_swap::ArcSwapOption::from_pointee(Some(model))),
            config: AgentConfig::default(),
            storage: None,
            context_provider: None,
            system_prompt_section: Some("## Profile\nYou are the orchestrator agent.".into()),
            system_prompt_identity: None,
            memory: Some(memory),
            tool_registry: Arc::new(registry),
            tasks: Arc::new(tokio::sync::RwLock::new(
                crate::tools::task_runtime::TaskStore::new(),
            )),
            event_tx: arc_swap::ArcSwapOption::empty(),
            persist_tx: std::sync::OnceLock::new(),
            plan: Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                agentik_types::AgentPlan::new(),
            ))),
        });

        let (internal_tx, _internal_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut session = Session::new_for_tests(shared, agentik_types::AgentPath::root());
        session.remember(Message::user("hello")).unwrap();
        session.agent_workflow(&internal_tx, None).await.unwrap();

        let calls = std::mem::take(&mut *captured.lock().unwrap());
        assert_eq!(calls.len(), 1, "exactly 1 model call expected");
        let system = calls[0].as_deref().expect("system prompt must be sent");
        let profile_at = system
            .find("## Profile\nYou are the orchestrator agent.")
            .expect("profile section must survive memory injection");
        let memory_at = system
            .find("## Persistent memory")
            .expect("memory section must be injected");
        assert!(
            system.contains("Prefer targeted tests."),
            "memory summary body must be in system"
        );
        assert!(
            profile_at < memory_at,
            "profile section must render before the memory section"
        );
    }

    /// Skill 指导随 system 一起到达模型边界:静态行为文本进 system
    /// (会话内稳定),与技能**索引**(spawn 时 extra section)互不干扰。
    #[tokio::test]
    async fn skill_guidance_ships_in_the_system_prompt() {
        use crate::testing::dummy_model_info;
        use agentik_sdk::model::Model;
        use agentik_sdk::provider::client::MockApiClient;
        use agentik_sdk::streaming::MessageStream;

        let captured: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
        let mut mock = MockApiClient::new();
        {
            let cap = Arc::clone(&captured);
            mock.expect_request_stream_with_system()
                .times(1)
                .returning(move |_, _, _, system| {
                    *cap.lock().unwrap() = system;
                    Ok(MessageStream::from_events(
                        Vec::new(),
                        Message::assistant_text("ok"),
                    ))
                });
        }
        let model = Model::with_client(dummy_model_info("skill-guide"), mock);

        let plan_state = Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
            agentik_types::AgentPlan::new(),
        )));
        let registry = crate::tools::ToolRegistry::new();
        let shared = Arc::new(AgentShared {
            id: Uuid::new_v4(),
            path: agentik_types::AgentPath::root(),
            config_json: serde_json::json!({}),
            model: Arc::new(arc_swap::ArcSwapOption::from_pointee(Some(model))),
            config: AgentConfig::default(),
            storage: None,
            context_provider: None,
            system_prompt_section: None,
            system_prompt_identity: None,
            memory: None,
            tool_registry: Arc::new(registry),
            tasks: Arc::new(tokio::sync::RwLock::new(
                crate::tools::task_runtime::TaskStore::new(),
            )),
            event_tx: arc_swap::ArcSwapOption::empty(),
            persist_tx: std::sync::OnceLock::new(),
            plan: plan_state,
        });

        let (internal_tx, _internal_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut session = Session::new_for_tests(shared, agentik_types::AgentPath::root());
        session.remember(Message::user("hello")).unwrap();
        session.agent_workflow(&internal_tx, None).await.unwrap();

        let system = captured.lock().unwrap().clone().expect("system captured");
        assert!(system.contains("## Skill library"), "{system}");
        for tool in [
            "skill_search",
            "skill_get",
            "evo_observe",
            "skill_propose",
            "skill_evolve",
        ] {
            assert!(system.contains(tool), "missing {tool} in guidance");
        }
    }

    /// Skills are guidance and tool users, not tool gatekeepers. Every tool
    /// registered on the session must reach the model request unchanged.
    #[tokio::test]
    async fn request_exposes_all_registered_tools() {
        use crate::testing::dummy_model_info;
        use agentik_sdk::model::Model;
        use agentik_sdk::provider::client::MockApiClient;
        use agentik_sdk::streaming::MessageStream;

        let captured_tools: Arc<std::sync::Mutex<Option<Vec<agentik_sdk::types::ToolDefinition>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let mut mock = MockApiClient::new();
        {
            let cap = Arc::clone(&captured_tools);
            mock.expect_request_stream_with_system()
                .times(1)
                .returning(move |_, tools, _, _| {
                    *cap.lock().unwrap() = Some(tools);
                    Ok(MessageStream::from_events(
                        Vec::new(),
                        Message::assistant_text("ok"),
                    ))
                });
        }
        let model = Model::with_client(dummy_model_info("all-tools"), mock);

        let plan_state = Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
            agentik_types::AgentPlan::new(),
        )));
        let tasks = Arc::new(tokio::sync::RwLock::new(
            crate::tools::task_runtime::TaskStore::new(),
        ));
        let mut registry = crate::tools::ToolRegistry::new();
        registry
            .register_all(crate::tools::task_registrations(Arc::clone(&tasks)))
            .unwrap();
        registry
            .register_all(crate::tools::plan_registrations(
                crate::tools::builtins::PlanHandle::new(
                    Arc::clone(&plan_state),
                    Uuid::new_v4(),
                    None,
                    None,
                ),
            ))
            .unwrap();

        let shared = Arc::new(AgentShared {
            id: Uuid::new_v4(),
            path: agentik_types::AgentPath::root(),
            config_json: serde_json::json!({}),
            model: Arc::new(arc_swap::ArcSwapOption::from_pointee(Some(model))),
            config: AgentConfig::default(),
            storage: None,
            context_provider: None,
            system_prompt_section: None,
            system_prompt_identity: None,
            memory: None,
            tool_registry: Arc::new(registry),
            tasks,
            event_tx: arc_swap::ArcSwapOption::empty(),
            persist_tx: std::sync::OnceLock::new(),
            plan: plan_state,
        });

        let (internal_tx, _internal_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut session = Session::new_for_tests(shared, agentik_types::AgentPath::root());
        let mut expected: Vec<String> = session
            .toolset
            .tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        expected.sort();
        assert!(expected.len() > 1, "test should register multiple tools");

        session.remember(Message::user("hello")).unwrap();
        session.agent_workflow(&internal_tx, None).await.unwrap();

        let mut actual: Vec<String> = captured_tools
            .lock()
            .unwrap()
            .take()
            .expect("tools captured")
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        actual.sort();
        assert_eq!(actual, expected);
    }
}
