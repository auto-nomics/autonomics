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
//! Each trigger emits a matching `CompactStart` / `CompactFinish` pair on
//! the event channel so the TUI can show progress.

use std::time::Instant;

use agentik_sdk::model::Model;
use agentik_sdk::types::messages::{ContentBlock, Message, Role};
use agentik_sdk::types::{AgentEvent, ToolDefinition};
use agentik_types::CompactEvent;
use chrono::Utc;

use crate::error::Result;
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
pub(super) const PRUNE_PROTECT_TOKENS: u64 = 40_000;
/// Only prune if at least this many tokens can be freed.
const PRUNE_MINIMUM_TOKENS: u64 = 20_000;
/// Chars per token heuristic (matching OpenCode's `Token.estimate()`).
const CHARS_PER_TOKEN: usize = 4;

impl Session {
    /// Compact conversation history in-place.
    ///
    /// Ported from Codex's compaction pipeline:
    ///
    /// 1. Selects a head/tail split point based on `keep_tokens` budget
    /// 2. Summarizes the head via an LLM call
    /// 3. Collects recent user messages from the head (up to a token budget)
    /// 4. Rebuilds `messages` = [preserved user messages] + [tail]
    /// 5. Appends the summary to `ancestor_summaries`
    ///
    /// Returns `Ok(true)` when compaction occurred, `Ok(false)` when the
    /// conversation is too short to compact.
    pub async fn compact(&mut self, model: &Model) -> Result<bool> {
        let selection = match select_for_compaction(&self.messages, DEFAULT_KEEP_TOKENS) {
            Some(sel) => sel,
            None => {
                tracing::debug!("nothing to compact — conversation is too short");
                return Ok(false);
            }
        };

        // Find previous summary (for anchored update)
        let previous_summary = self.ancestor_summaries.last().cloned();

        // Build the summarization prompt
        let prompt_text = build_compaction_prompt(&selection.head, previous_summary.as_deref());

        let messages: Vec<Message> = vec![Message::system(prompt_text)];
        let response = model
            .request(messages, &Vec::<ToolDefinition>::new())
            .await?;
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

        // ── Preserve recent user messages from the compacted head ──
        //
        // Ported from Codex: carry forward user messages (up to
        // COMPACT_USER_MESSAGE_MAX_TOKENS tokens) from the compacted head so
        // the model retains direct access to original user intent without
        // relying solely on the summary.
        let head_messages = &self.messages[..selection.tail_message_start];
        let preserved_user_msgs =
            collect_recent_user_messages(head_messages, COMPACT_USER_MESSAGE_MAX_TOKENS);

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
        self.ancestor_summaries.push(formatted_summary);

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

        tracing::debug!(
            messages = self.messages.len(),
            ancestors = self.ancestor_summaries.len(),
            "session compacted in-place"
        );

        Ok(true)
    }

    /// Manually trigger compaction on the active session.
    ///
    /// Unlike the automatic compaction in `request()` / `agent_workflow()`,
    /// this is initiated by the user (via a command) rather than by context
    /// pressure. Emits Compact start/finish events so the TUI can show
    /// progress. Does **not** start a new LLM turn — the caller can inject a
    /// follow-up message if it wants the agent to resume work.
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

        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Compacting);
        self.shared.send_event(AgentEvent::Compact {
            event: CompactEvent::CompactStart { ts: Utc::now() },
        });

        match self.compact(model.as_ref()).await {
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

        self.shared.send_event(AgentEvent::Compact {
            event: CompactEvent::CompactFinish { ts: Utc::now() },
        });
        self.set_lifecycle(lifecycle_before_compact);
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
