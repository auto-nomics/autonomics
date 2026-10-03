//! Context compaction: head/tail selection, summarization, and the token
//! estimation / pruning helpers shared with context rendering.
//!
//! Compaction is triggered from three places (see `workflow.rs` and
//! `do_manual_compact` below):
//!
//! 1. Manually via the `InternalEvent::Compact` command.
//! 2. Mid-turn, after tool execution, before the follow-up LLM call.
//! 3. Pre-request, when the estimated context exceeds the auto-compact
//!    threshold of the model's context window.
//!
//! All three share [`Session::compact`], which owns the progress-event
//! choreography: `CompactStart` (carrying a `CompactPlan`) →
//! `CompactPhase(Summarizing)` → throttled `CompactSummaryDelta` stream →
//! `CompactPhase(Rebuilding)` → `CompactFinish` (carrying `CompactStats`).
//! The Start/Finish pair always closes — on error and on the
//! too-short-to-compact skip — so frontends never hang in a compacting
//! state.

use std::time::{Duration, Instant};

use agentik_sdk::model::Model;
use agentik_sdk::types::messages::{ContentBlock, Message, Role};
use agentik_sdk::types::{AgentEvent, AnthropicError, ToolDefinition};
use agentik_types::CompactEvent;
use agentik_types::{CompactPhase, CompactPlan, CompactStats, CompactTrigger};
use chrono::Utc;
use futures::StreamExt;

use crate::error::{AgentError, Result};
use crate::message_ext::AgentMessageExt;
use crate::prompt::compact;
use crate::storage::PersistOp;

use super::{Session, SessionState};

// ── Compaction constants ───────────────────────────────────────────

/// Maximum characters per tool output when serializing for summarization.
const TOOL_OUTPUT_MAX_CHARS: usize = 2_000;
/// Default tokens to preserve in the "recent" tail during compaction.
pub const DEFAULT_KEEP_TOKENS: u64 = 8_000;
/// Maximum tokens of recent user messages to carry forward into compacted
/// history (ported from Codex `COMPACT_USER_MESSAGE_MAX_TOKENS`).
const COMPACT_USER_MESSAGE_MAX_TOKENS: u64 = 20_000;
/// Minimum tokens of recent tool output to protect from pruning.
///
/// Temporarily unused: render-time pruning invalidated prompt-cache prefixes.
#[allow(dead_code)]
pub(super) const PRUNE_PROTECT_TOKENS: u64 = 40_000;
/// Only prune if at least this many tokens can be freed.
#[allow(dead_code)]
const PRUNE_MINIMUM_TOKENS: u64 = 20_000;
/// Chars per token heuristic (matching OpenCode's `Token.estimate()`).
const CHARS_PER_TOKEN: usize = 4;
/// Flush a buffered summary delta once it reaches this many characters.
const COMPACT_DELTA_FLUSH_CHARS: usize = 120;
/// …or once this much time has passed since the last flush.
const COMPACT_DELTA_FLUSH_INTERVAL: Duration = Duration::from_millis(200);
/// Upper bound on waiting for the summary stream's final message.
const COMPACT_FINAL_MESSAGE_TIMEOUT: Duration = Duration::from_secs(5);

impl Session {
    /// Compact conversation history in-place.
    ///
    /// Ported from Codex's compaction pipeline:
    ///
    /// 1. Selects a head/tail split point based on `keep_tokens` budget
    /// 2. Summarizes the head via a streaming LLM call (deltas forwarded)
    /// 3. Collects recent user messages from the head (up to a token budget)
    /// 4. Rebuilds `messages` = [preserved user messages] + [tail]
    /// 5. Appends the summary to `ancestor_summaries`
    ///
    /// Emits the full `CompactEvent` progress sequence and restores the
    /// lifecycle to `lifecycle_after` on every exit path.
    ///
    /// Returns `Ok(true)` when compaction occurred, `Ok(false)` when the
    /// conversation is too short to compact.
    pub async fn compact(
        &mut self,
        model: &Model,
        trigger: CompactTrigger,
        lifecycle_after: agentik_types::AgentLifecycleStatus,
    ) -> Result<bool> {
        let started = Instant::now();
        let messages_before = self.messages.len();

        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Compacting);

        // ── Selecting (local, ms) — outcome folded into the Start plan ──
        let selection = match select_for_compaction(&self.messages, DEFAULT_KEEP_TOKENS) {
            Some(sel) => sel,
            None => {
                // Too short to compact. Still close the Start/Finish pair so
                // frontends can dismiss their progress UI deterministically.
                self.send_compact_event(CompactEvent::CompactStart {
                    ts: Utc::now(),
                    plan: None,
                });
                self.send_compact_event(CompactEvent::CompactFinish {
                    ts: Utc::now(),
                    stats: None,
                    error: None,
                });
                tracing::debug!("nothing to compact — conversation is too short");
                self.set_lifecycle(lifecycle_after);
                return Ok(false);
            }
        };

        let plan = CompactPlan {
            trigger,
            head_messages: selection.tail_message_start,
            head_tokens: estimate_tokens(&selection.head),
            tail_messages: messages_before - selection.tail_message_start,
        };
        self.send_compact_event(CompactEvent::CompactStart {
            ts: Utc::now(),
            plan: Some(plan.clone()),
        });

        // ── Summarizing (LLM call — the slow part, streamed) ──
        self.send_compact_event(CompactEvent::CompactPhase {
            ts: Utc::now(),
            phase: CompactPhase::Summarizing,
        });

        // Find previous summary (for anchored update)
        let previous_summary = self.ancestor_summaries.last().cloned();
        let prompt_text = build_compaction_prompt(&selection.head, previous_summary.as_deref());
        let messages: Vec<Message> = vec![Message::system(prompt_text)];

        let response = match self.summarize_head(model, messages).await {
            Ok(response) => response,
            Err(e) => {
                // Close the event pair on failure — a Finish-less Start
                // would leave frontends stuck compacting forever.
                self.send_compact_event(CompactEvent::CompactFinish {
                    ts: Utc::now(),
                    stats: None,
                    error: Some(format!("{e}")),
                });
                self.set_lifecycle(lifecycle_after);
                return Err(e);
            }
        };

        let usage = response.usage.clone().unwrap_or_default();
        self.telemetry.record_usage(&usage);
        self.last_active = chrono::Utc::now().timestamp_millis();
        self.persist_telemetry();

        // Extract the summary text from the LLM response
        let raw_summary: String = response
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<String>>()
            .join("");

        let formatted_summary = compact::format_compact_summary(&raw_summary);

        tracing::info!(
            summary_len = formatted_summary.len(),
            "compaction summary generated"
        );

        // ── Rebuilding (local, ms) ──
        self.send_compact_event(CompactEvent::CompactPhase {
            ts: Utc::now(),
            phase: CompactPhase::Rebuilding,
        });

        // ── Preserve recent user messages from the compacted head ──
        //
        // Ported from Codex: carry forward user messages (up to
        // COMPACT_USER_MESSAGE_MAX_TOKENS tokens) from the compacted head so
        // the model retains direct access to original user intent without
        // relying solely on the summary.
        let head_messages = &self.messages[..selection.tail_message_start];
        let preserved_user_msgs =
            collect_recent_user_messages(head_messages, COMPACT_USER_MESSAGE_MAX_TOKENS);
        let preserved_tokens: u64 = preserved_user_msgs
            .iter()
            .map(estimate_message_tokens)
            .sum();

        // Retain the tail (recent messages from the split point onward)
        let tail = self.messages[selection.tail_message_start..].to_vec();

        // Archive the full user-facing transcript before the active model
        // context is replaced.
        if let Some(tx) = &self.persist_tx {
            let _ = tx.send(PersistOp::ArchiveTranscript {
                session_id: self.id,
                messages: self.messages.clone(),
            });
        }

        // New message list = [preserved user messages] + [tail]
        let mut new_messages = preserved_user_msgs;
        new_messages.extend(tail);
        self.messages = new_messages;

        // Reset token budget: next estimate will be fresh
        self.token_budget = crate::agent::TokenBudget::default();

        // Store the latest summary on the session (primary) and append to
        // the ancestor list (for context rendering).
        self.summary = Some(formatted_summary.clone());
        self.ancestor_summaries.push(formatted_summary.clone());

        // Persist the compacted state to the WAL so a crash after compaction
        // doesn't restore stale pre-compaction messages.
        if let Some(tx) = &self.persist_tx {
            let _ = tx.send(PersistOp::ReplaceSessionState {
                agent_id: self.shared.id,
                session_id: self.id,
                state: SessionState {
                    messages: self.messages.clone(),
                    summary: self.summary.clone(),
                    ancestor_summaries: self.ancestor_summaries.clone(),
                    telemetry: self.telemetry,
                },
            });
        }

        // The head/tail split can leave orphaned tool_results whose
        // tool_use was summarised away. Run the Anthropic-invariant
        // sanitizer and, if it repaired anything, re-emit a fresh WAL
        // ReplaceSessionState so the persisted shape is also clean.
        self.sanitize_and_persist();

        let summary_tokens = estimate_tokens(&formatted_summary);
        let stats = CompactStats {
            messages_before,
            messages_after: self.messages.len(),
            summary_tokens,
            freed_tokens: plan
                .head_tokens
                .saturating_sub(preserved_tokens)
                .saturating_sub(summary_tokens),
            duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            usage,
        };
        self.send_compact_event(CompactEvent::CompactFinish {
            ts: Utc::now(),
            stats: Some(stats),
            error: None,
        });
        self.set_lifecycle(lifecycle_after);

        tracing::debug!(
            messages = self.messages.len(),
            ancestors = self.ancestor_summaries.len(),
            "session compacted in-place"
        );

        Ok(true)
    }

    /// Summarize the compacted head via a streaming LLM call, forwarding
    /// text deltas as throttled `CompactSummaryDelta` events.
    ///
    /// Falls back to a plain non-streaming request when the provider
    /// rejects streaming (some gateways do for tool-less requests) — the
    /// frontends then simply lose the live preview, not the compaction.
    async fn summarize_head(&mut self, model: &Model, messages: Vec<Message>) -> Result<Message> {
        let no_tools: Vec<ToolDefinition> = Vec::new();
        let mut stream = match model.request_stream(messages.clone(), &no_tools).await {
            Ok(stream) => stream,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "streaming summarization unavailable; falling back to non-streaming request"
                );
                return model.request(messages, &no_tools).await.map_err(Into::into);
            }
        };

        let mut throttle = DeltaThrottle::new();
        loop {
            // Race the next stream event against cancellation so Ctrl+C
            // interrupts a slow summarization immediately.
            let event = tokio::select! {
                ev = stream.next() => match ev {
                    Some(ev) => ev,
                    None => break,
                },
                _ = self.cancel_token.cancelled() => {
                    tracing::info!("compaction summarization cancelled by user");
                    stream.abort();
                    return Err(AgentError::Cancelled);
                }
            };

            let stream_event = match event {
                Ok(e) => e,
                Err(e) => match &e {
                    AnthropicError::StreamError(msg) if msg.starts_with("Stream lagged:") => {
                        tracing::debug!("skipping lagged compaction stream event: {e}");
                        continue;
                    }
                    _ => {
                        tracing::warn!(
                            "compaction stream error: {e}; breaking stream loop, awaiting final message"
                        );
                        break;
                    }
                },
            };

            if let Some(AgentEvent::TextDelta(text)) = AgentEvent::from_stream_event(&stream_event)
            {
                throttle.push(&text, |flushed| {
                    self.send_compact_event(CompactEvent::CompactSummaryDelta {
                        ts: Utc::now(),
                        text: flushed,
                    });
                });
            }
        }
        tracing::info!("compaction summary stream ended, awaiting final_message");

        let response = tokio::time::timeout(COMPACT_FINAL_MESSAGE_TIMEOUT, stream.final_message())
            .await
            .map_err(|e| AgentError::WorkflowFailed {
                iteration: 0,
                error: Box::new(AgentError::MissingConfig(format!(
                    "compaction final_message() timed out: {e}"
                ))),
            })??;

        // Drain the throttle tail so the live preview ends on the complete
        // summary text rather than a 120-char boundary.
        throttle.flush(|flushed| {
            self.send_compact_event(CompactEvent::CompactSummaryDelta {
                ts: Utc::now(),
                text: flushed,
            });
        });

        Ok(response)
    }

    /// Emit a compaction progress event on the observation channel.
    fn send_compact_event(&self, event: CompactEvent) {
        self.shared.send_event(AgentEvent::Compact { event });
    }

    /// Manually trigger compaction on the active session.
    ///
    /// Unlike the automatic compaction in `request()` / `agent_workflow()`,
    /// this is initiated by the user (via a command) rather than by context
    /// pressure. The lifecycle choreography and the Start/Finish event pair
    /// (including the failure path) live inside `compact`. Does **not**
    /// start a new LLM turn — the caller can inject a follow-up message if
    /// it wants the agent to resume work.
    pub(super) async fn do_manual_compact(&mut self) {
        // A compact command may be queued while a turn is active. Restore
        // that lifecycle afterward so an in-flight stream keeps driving the UI.
        let lifecycle_before_compact = *self.lifecycle.status();
        let model = match self.shared.model.load_full() {
            Some(m) => m,
            None => {
                tracing::warn!("manual compact requested but no model is configured");
                self.shared.send_event(AgentEvent::Error(
                    "No model configured for compaction".into(),
                ));
                return;
            }
        };
        let timing_was_active = self.turn_timing.is_active();
        let standalone_started = Instant::now();

        match self
            .compact(
                model.as_ref(),
                CompactTrigger::Manual,
                lifecycle_before_compact,
            )
            .await
        {
            Ok(compacted) => {
                if compacted {
                    tracing::info!("manual compaction completed");
                } else {
                    tracing::info!("manual compaction skipped — conversation too short");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "manual compaction failed");
                self.shared
                    .send_event(AgentEvent::Error(format!("Compaction failed: {e}")));
            }
        }
        if !timing_was_active {
            self.telemetry.record_elapsed(
                standalone_started
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            );
            self.last_active = chrono::Utc::now().timestamp_millis();
            self.persist_telemetry();
        }
    }
}

// ── Summary-delta throttling ────────────────────────────────────────

/// Coalesces summary text deltas before they reach the event channel.
///
/// Streaming deltas arrive per-token; forwarding each one would flood the
/// SSE hub (and its replay ring used for `Last-Event-ID` catch-up). A
/// flush fires when the buffer reaches [`COMPACT_DELTA_FLUSH_CHARS`] or
/// [`COMPACT_DELTA_FLUSH_INTERVAL`] has elapsed since the last flush,
/// whichever comes first; [`DeltaThrottle::flush`] drains the tail
/// unconditionally when summarization ends.
struct DeltaThrottle {
    buf: String,
    last_flush: Instant,
}

impl DeltaThrottle {
    fn new() -> Self {
        Self {
            buf: String::new(),
            last_flush: Instant::now(),
        }
    }

    fn push(&mut self, chunk: &str, mut emit: impl FnMut(String)) {
        self.buf.push_str(chunk);
        if self.buf.len() >= COMPACT_DELTA_FLUSH_CHARS
            || self.last_flush.elapsed() >= COMPACT_DELTA_FLUSH_INTERVAL
        {
            emit(std::mem::take(&mut self.buf));
            self.last_flush = Instant::now();
        }
    }

    /// Drain whatever remains (end-of-stream tail).
    fn flush(&mut self, mut emit: impl FnMut(String)) {
        if !self.buf.is_empty() {
            emit(std::mem::take(&mut self.buf));
        }
    }
}

// ── Compaction helper functions (moved from memory.rs) ─────────────

/// Result of the head/tail sliding-window selection for compaction.
struct CompactionSelection {
    /// Serialized text of old messages to be summarized.
    head: String,
    /// Index at which the tail starts within the message list.
    tail_message_start: usize,
}

/// Estimate token count for a string using the chars/4 heuristic.
fn estimate_tokens(text: &str) -> u64 {
    (text.len() / CHARS_PER_TOKEN) as u64
}

/// Estimate token count for a single message by summing its content blocks.
///
/// This is the single source of truth for token estimation across the
/// agent — `TokenBudget` (in `agent.rs`) and compaction helpers both
/// delegate here.
pub fn estimate_message_tokens(msg: &Message) -> u64 {
    let text: String = msg
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::ToolUse { name, input, .. } => {
                format!(
                    "[tool:{name} {}",
                    serde_json::to_string(input).unwrap_or_default()
                )
            }
            ContentBlock::ToolResult { content, .. } => {
                content.as_deref().unwrap_or("").to_string()
            }
            ContentBlock::Thinking { thinking, .. } => thinking.clone(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    estimate_tokens(&text)
}

/// Serialize a list of messages into plain text for the summarization LLM.
fn serialize_messages(messages: &[Message]) -> String {
    let mut parts = Vec::new();

    for msg in messages {
        match &msg.role {
            Role::User => {
                let text_blocks: Vec<String> = msg
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect();
                let text = text_blocks.join("\n");
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }

                for block in &msg.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id: _,
                        content,
                        is_error,
                    } = block
                    {
                        let content_text = content.as_deref().unwrap_or("");
                        let truncated = truncate_for_compact(content_text);
                        let prefix = if is_error.unwrap_or(false) {
                            "[Tool error]"
                        } else {
                            "[Tool result]"
                        };
                        parts.push(format!("{prefix}: {truncated}"));
                    }
                }
            }
            Role::Assistant => {
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                parts.push(format!("[Assistant]: {text}"));
                            }
                        }
                        ContentBlock::ToolUse { name, input, .. } => {
                            let input_str = serde_json::to_string(input).unwrap_or_default();
                            parts.push(format!("[Assistant tool call]: {name}({input_str})"));
                        }
                        ContentBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                            parts.push(format!("[Assistant reasoning]: {thinking}"));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    parts.join("\n\n")
}

/// Truncate a tool output string for compaction summarization input.
fn truncate_for_compact(text: &str) -> String {
    if text.len() <= TOOL_OUTPUT_MAX_CHARS {
        return text.to_string();
    }
    // Floor to the nearest char boundary so we never slice mid-codepoint
    // (panics on multi-byte UTF-8 content — e.g. Chinese text, emoji).
    let mut end = TOOL_OUTPUT_MAX_CHARS;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated {} chars]", &text[..end], text.len() - end)
}

/// Select a split point in the conversation for compaction.
///
/// Walks backwards from the most recent messages, accumulating tokens
/// until `keep_tokens` is exhausted. Everything before the split is
/// "head" (to be summarized), everything after is "recent" (preserved).
///
/// Returns `None` if the conversation is too short to need compaction.
fn select_for_compaction(messages: &[Message], keep_tokens: u64) -> Option<CompactionSelection> {
    if messages.is_empty() {
        return None;
    }

    // Walk backwards to find the split point
    let mut accumulated: u64 = 0;
    let mut split_index = 0;

    for (i, msg) in messages.iter().enumerate().rev() {
        accumulated += estimate_message_tokens(msg);
        if accumulated > keep_tokens {
            split_index = i;
            break;
        }
    }

    // If we never exceeded keep_tokens, nothing to compact.
    if split_index == 0 && accumulated <= keep_tokens {
        return None;
    }

    // Don't compact if the head is empty (everything fits in the tail).
    if split_index == 0 {
        return None;
    }

    let head_msgs = &messages[..split_index];
    let head = serialize_messages(head_msgs);

    Some(CompactionSelection {
        head,
        tail_message_start: split_index,
    })
}

/// Collect recent user messages (walking backwards from the end of `messages`)
/// until `max_tokens` is exhausted.
///
/// Ported from Codex's `build_compacted_history_with_limit`: preserves the
/// original user intent verbatim in the compacted history so the model can
/// reference exact wording without relying on the summary.
fn collect_recent_user_messages(messages: &[Message], max_tokens: u64) -> Vec<Message> {
    let mut selected: Vec<Message> = Vec::new();
    let mut remaining = max_tokens;

    for msg in messages.iter().rev() {
        if remaining == 0 {
            break;
        }
        if msg.role != Role::User {
            continue;
        }
        // Skip tool-result-only user messages (they're not real user intent)
        let has_text = msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if !text.is_empty()));
        if !has_text {
            continue;
        }
        let tokens = estimate_message_tokens(msg);
        if tokens > remaining {
            break;
        }
        remaining = remaining.saturating_sub(tokens);
        selected.push(msg.clone());
    }

    selected.reverse();
    selected
}

/// Prune old tool outputs from a message list by replacing their content
/// with a placeholder. Protects the most recent `protect_tokens` worth
/// of tool output content.
///
/// Temporarily unused: render-time pruning invalidated prompt-cache prefixes.
#[allow(dead_code)]
pub(super) fn prune_old_tool_outputs(messages: &mut [Message], protect_tokens: u64) {
    if messages.is_empty() {
        return;
    }

    let mut accumulated: u64 = 0;
    let mut prunable_total: u64 = 0;

    // First pass: count what's prunable
    for msg in messages.iter().rev() {
        for block in &msg.content {
            if let ContentBlock::ToolResult { content, .. } = block {
                let content_len = content.as_deref().map(|c| c.len()).unwrap_or(0);
                let tokens = (content_len / CHARS_PER_TOKEN) as u64;
                accumulated += tokens;
                if accumulated > protect_tokens {
                    prunable_total += tokens;
                }
            }
        }
    }

    if prunable_total < PRUNE_MINIMUM_TOKENS {
        return;
    }

    // Second pass: actually prune
    let mut accumulated: u64 = 0;
    for msg in messages.iter_mut().rev() {
        for block in &mut msg.content {
            if let ContentBlock::ToolResult { content, .. } = block {
                let content_len = content.as_deref().map(|c| c.len()).unwrap_or(0);
                let tokens = (content_len / CHARS_PER_TOKEN) as u64;
                accumulated += tokens;
                if accumulated > protect_tokens {
                    *content = Some("[Old tool result content cleared]".to_string());
                }
            }
        }
    }
}

/// Format a compaction summary into a user message.
///
/// Uses Codex's summary-prefix style: contextualizes the summary as a
/// handoff from a previous model's work.
pub(super) fn format_checkpoint_message(summary: &str) -> String {
    format!(
        "Another language model started to solve this problem and produced a summary \
         of its thinking process. Use this to build on the work that has already been \
         done and avoid duplicating work. Here is the summary:\n\n{summary}"
    )
}

/// Build the summarization prompt (ported from Codex's concise handoff style).
///
/// Codex uses a focused "context checkpoint compaction" prompt rather than a
/// verbose multi-section template. This produces shorter summaries and fewer
/// wasted output tokens.
fn build_compaction_prompt(head: &str, previous_summary: Option<&str>) -> String {
    let task = if let Some(prev) = previous_summary {
        format!(
            "Update the existing context checkpoint using the conversation history below. \
             Preserve still-true details, remove stale ones, and merge in new facts.\n\n\
             <previous-summary>\n{prev}\n</previous-summary>"
        )
    } else {
        "You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary \
         for another LLM that will resume the task."
            .to_string()
    };

    format!(
        "{task}\n\n\
         Include:\n\
         - Current progress and key decisions made\n\
         - Important context, constraints, or user preferences\n\
         - What remains to be done (clear next steps)\n\
         - Any critical data, examples, or references needed to continue\n\
         - All user messages verbatim (not tool results)\n\n\
         Be concise, structured, and focused on helping the next LLM seamlessly \
         continue the work. Respond with plain text only — do NOT call any tools.\n\n\
         Conversation to summarize:\n{head}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::dummy_model_info;
    use agentik_sdk::model::Model;
    use agentik_sdk::provider::client::MockApiClient;
    use agentik_sdk::streaming::MessageStream;
    use agentik_sdk::types::messages::ContentBlock;
    use agentik_sdk::types::{ContentBlockDelta, MessageStreamEvent};
    use agentik_types::AgentPath;
    use std::sync::Arc;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
    use uuid::Uuid;

    /// Build an `AgentShared` whose event channel drains into `tx` — the
    /// compaction progress events are the observable under test.
    fn shared_with_model_and_events(
        model: Model,
        tx: UnboundedSender<AgentEvent>,
    ) -> Arc<super::super::AgentShared> {
        Arc::new(super::super::AgentShared {
            id: Uuid::new_v4(),
            path: AgentPath::root(),
            config_json: serde_json::json!({}),
            model: Arc::new(arc_swap::ArcSwapOption::from_pointee(Some(model))),
            config: crate::agent::AgentConfig::default(),
            storage: None,
            context_provider: None,
            system_prompt_section: None,
            system_prompt_identity: None,
            memory: None,
            tool_registry: Arc::new(crate::tools::ToolRegistry::new()),
            tasks: Arc::new(tokio::sync::RwLock::new(
                crate::tools::task_runtime::TaskStore::new(),
            )),
            event_tx: arc_swap::ArcSwapOption::from_pointee(Some(tx)),
            persist_tx: std::sync::OnceLock::new(),
            plan: Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                agentik_types::AgentPlan::new(),
            ))),
        })
    }

    /// Drain the observation channel, keeping only `AgentEvent::Compact`.
    fn drain_compact_events(rx: &mut UnboundedReceiver<AgentEvent>) -> Vec<CompactEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AgentEvent::Compact { event } = ev {
                out.push(event);
            }
        }
        out
    }

    /// A mock model whose summarization stream emits three text deltas
    /// (60 + 60 + 60 chars) and a final summary message.
    fn streaming_summary_model(summary: &'static str) -> Model {
        let mut mock = MockApiClient::new();
        mock.expect_request_stream()
            .times(1)
            .returning(move |_m, _t, _i| {
                Ok(MessageStream::from_events(
                    vec![
                        MessageStreamEvent::ContentBlockStart {
                            content_block: ContentBlock::Text {
                                text: String::new(),
                            },
                            index: 0,
                        },
                        MessageStreamEvent::ContentBlockDelta {
                            delta: ContentBlockDelta::TextDelta {
                                text: "a".repeat(60),
                            },
                            index: 0,
                        },
                        MessageStreamEvent::ContentBlockDelta {
                            delta: ContentBlockDelta::TextDelta {
                                text: "b".repeat(60),
                            },
                            index: 0,
                        },
                        MessageStreamEvent::ContentBlockDelta {
                            delta: ContentBlockDelta::TextDelta {
                                text: "c".repeat(60),
                            },
                            index: 0,
                        },
                        MessageStreamEvent::ContentBlockStop { index: 0 },
                    ],
                    Message::assistant_text(summary),
                ))
            });
        Model::with_client(dummy_model_info("compact-test"), mock)
    }

    /// Alternate ~1.1k-char user/assistant messages so the conversation
    /// crosses the compaction threshold (DEFAULT_KEEP_TOKENS = 8k).
    fn long_conversation(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| {
                let text = format!("message {i:03} {}", "x".repeat(1080));
                if i % 2 == 0 {
                    Message::user(text)
                } else {
                    Message::assistant_text(text)
                }
            })
            .collect()
    }

    /// The full progress-event choreography: Start(plan) →
    /// Phase(Summarizing) → throttled deltas → Phase(Rebuilding) →
    /// Finish(stats), plus the in-place rebuild and lifecycle restore.
    #[tokio::test]
    async fn compact_emits_full_progress_sequence() {
        let model = streaming_summary_model("## Progress\n- did the thing");
        let (tx, mut rx) = unbounded_channel();
        let shared = shared_with_model_and_events(model.clone(), tx);
        let mut session = super::Session::new_for_tests(shared, AgentPath::root());
        for msg in long_conversation(40) {
            session.remember(msg).unwrap();
        }

        let compacted = session
            .compact(
                &model,
                CompactTrigger::Manual,
                agentik_types::AgentLifecycleStatus::Idle,
            )
            .await
            .expect("compaction succeeds");
        assert!(compacted);

        // ── Event sequence ──
        let events = drain_compact_events(&mut rx);
        assert_eq!(events.len(), 6, "got {events:?}");
        assert!(
            matches!(&events[0], CompactEvent::CompactStart { plan: Some(p), .. }
            if p.trigger == CompactTrigger::Manual
                && p.head_messages > 0 && p.head_messages < 40
                && p.head_tokens > 0 && p.tail_messages > 0),
            "plan should describe the split: {events:?}"
        );
        assert!(matches!(
            &events[1],
            CompactEvent::CompactPhase {
                phase: CompactPhase::Summarizing,
                ..
            }
        ));
        // Deltas are coalesced: 60+60 chars cross the 120-char threshold →
        // one flush; the trailing 60 chars drain via the final flush.
        assert!(
            matches!(&events[2], CompactEvent::CompactSummaryDelta { text, .. }
            if text == &format!("{}{}", "a".repeat(60), "b".repeat(60)))
        );
        assert!(
            matches!(&events[3], CompactEvent::CompactSummaryDelta { text, .. }
            if text == &"c".repeat(60))
        );
        assert!(matches!(
            &events[4],
            CompactEvent::CompactPhase {
                phase: CompactPhase::Rebuilding,
                ..
            }
        ));
        match &events[5] {
            CompactEvent::CompactFinish {
                stats: Some(stats),
                error: None,
                ..
            } => {
                assert_eq!(stats.messages_before, 40);
                assert!(stats.messages_after < 40, "head should shrink: {stats:?}");
                assert!(stats.summary_tokens > 0);
                assert!(
                    stats.freed_tokens > 0,
                    "net context should shrink: {stats:?}"
                );
            }
            other => panic!("expected stats-carrying Finish, got {other:?}"),
        }

        // ── In-place effects ──
        assert_eq!(
            session.lifecycle_status(),
            agentik_types::AgentLifecycleStatus::Idle
        );
        assert_eq!(session.ancestor_summaries.len(), 1);
        assert!(session.summary.is_some());
        assert!(session.messages.len() < 40);
    }

    /// A conversation too short to compact still closes the Start/Finish
    /// pair so frontends can dismiss their progress UI deterministically.
    /// The mock carries no expectations — any LLM call would panic, which
    /// doubles as the "no summarization happened" assertion.
    #[tokio::test]
    async fn compact_short_conversation_closes_event_pair() {
        let model = Model::with_client(dummy_model_info("compact-skip"), MockApiClient::new());
        let (tx, mut rx) = unbounded_channel();
        let shared = shared_with_model_and_events(model.clone(), tx);
        let mut session = super::Session::new_for_tests(shared, AgentPath::root());
        session.remember(Message::user("hi")).unwrap();

        let compacted = session
            .compact(
                &model,
                CompactTrigger::Manual,
                agentik_types::AgentLifecycleStatus::Idle,
            )
            .await
            .expect("short conversation is not an error");
        assert!(!compacted);

        let events = drain_compact_events(&mut rx);
        assert!(
            matches!(
                &events[..],
                [
                    CompactEvent::CompactStart { plan: None, .. },
                    CompactEvent::CompactFinish {
                        stats: None,
                        error: None,
                        ..
                    },
                ]
            ),
            "got {events:?}"
        );
        assert_eq!(
            session.lifecycle_status(),
            agentik_types::AgentLifecycleStatus::Idle
        );
    }

    /// When the provider rejects streaming, compaction falls back to a
    /// plain request: no deltas, but the full Start/Phase/Finish sequence
    /// still holds.
    #[tokio::test]
    async fn compact_falls_back_when_streaming_unavailable() {
        let mut mock = MockApiClient::new();
        mock.expect_request_stream()
            .times(1)
            .returning(|_m, _t, _i| {
                Err(agentik_sdk::AnthropicError::BadRequest {
                    message: "streaming unsupported".into(),
                    status: 400,
                })
            });
        mock.expect_request()
            .times(1)
            .returning(|_m, _t, _i| Ok(Message::assistant_text("fallback summary")));
        let model = Model::with_client(dummy_model_info("compact-fallback"), mock);

        let (tx, mut rx) = unbounded_channel();
        let shared = shared_with_model_and_events(model.clone(), tx);
        let mut session = super::Session::new_for_tests(shared, AgentPath::root());
        for msg in long_conversation(40) {
            session.remember(msg).unwrap();
        }

        let compacted = session
            .compact(
                &model,
                CompactTrigger::Manual,
                agentik_types::AgentLifecycleStatus::Idle,
            )
            .await
            .expect("fallback request succeeds");
        assert!(compacted);

        let events = drain_compact_events(&mut rx);
        // Start → Phase(Summarizing) → (no deltas) → Phase(Rebuilding) → Finish
        assert_eq!(events.len(), 4, "got {events:?}");
        assert!(matches!(
            &events[1],
            CompactEvent::CompactPhase {
                phase: CompactPhase::Summarizing,
                ..
            }
        ));
        assert!(matches!(
            &events[3],
            CompactEvent::CompactFinish {
                stats: Some(_),
                error: None,
                ..
            }
        ));
    }

    // ── DeltaThrottle ────────────────────────────────────────────────

    #[test]
    fn delta_throttle_flushes_on_char_threshold_then_tail() {
        let mut t = DeltaThrottle::new();
        let mut out = Vec::new();

        t.push(&"a".repeat(60), |s| out.push(s));
        assert!(out.is_empty(), "below threshold and interval — buffered");

        t.push(&"b".repeat(60), |s| out.push(s));
        assert_eq!(
            out,
            vec![format!("{}{}", "a".repeat(60), "b".repeat(60))],
            "120 chars crossed the flush threshold"
        );

        t.push(&"c".repeat(10), |s| out.push(s));
        assert_eq!(out.len(), 1, "tail below threshold stays buffered");

        t.flush(|s| out.push(s));
        assert_eq!(out[1], "c".repeat(10), "flush drains the tail");
        t.flush(|s| out.push(s));
        assert_eq!(out.len(), 2, "flush on an empty buffer is a no-op");
    }

    #[test]
    fn delta_throttle_flushes_on_interval() {
        // Backdate the last flush beyond the interval — same-module test
        // can construct the private field directly, avoiding a wall-clock
        // sleep in the test suite.
        let mut t = DeltaThrottle {
            buf: String::new(),
            last_flush: Instant::now() - COMPACT_DELTA_FLUSH_INTERVAL - Duration::from_millis(1),
        };
        let mut out = Vec::new();
        t.push("tiny", |s| out.push(s));
        assert_eq!(out, vec!["tiny".to_string()], "interval elapsed — flushed");
    }

    #[test]
    fn delta_throttle_never_splits_utf8() {
        // Chars (not bytes) are appended whole; flush emits the entire
        // buffer, so multi-byte content is never sliced mid-codepoint.
        let mut t = DeltaThrottle::new();
        let mut out = Vec::new();
        let cjk = "摘要".repeat(80); // 3-byte codepoints, > threshold in bytes
        t.push(&cjk, |s| out.push(s));
        assert_eq!(out, vec![cjk]);
    }
}
