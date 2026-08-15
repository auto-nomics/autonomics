//! Session — a single conversation within an Agent.
//!
//! An [`Agent`](crate::Agent) owns one or more `Session`s. Each Session has
//! independent conversation memory, lifecycle, tool execution state, and
//! cancellation token, but shares the model, tool registry, storage, and
//! system-prompt configuration with its siblings.
//!
//! Currently only one session is *active* at a time (single-active model).
//! Switching sessions pauses the current one and activates the target.
//!
//! ## Merged Memory Model
//!
//! Session directly holds its conversation data (`messages`, `summary`,
//! `ancestor_summaries`). Compaction operates in-place: the head messages are
//! summarized into `ancestor_summaries`, and only the recent tail is retained
//! in `messages`. This eliminates the former `Memory`/`MemoryItem` layer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use agentik_sdk::model::Model;
use agentik_sdk::types::messages::{ContentBlock, Message, Role};
use agentik_sdk::types::tools::ToolUse;
use agentik_sdk::types::{AgentEvent, AnthropicError, ToolDefinition};
use agentik_types::{AgentPlan, CompactEvent, PlanUpdate, SessionInfo, TurnExecutionStatus};
use arc_swap::{ArcSwap, ArcSwapOption};
use chrono::Utc;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use tracing::{Level, span};
use uuid::Uuid;

use crate::agent::{AgentConfig, InternalEvent, TokenBudget};
use crate::context::ContextProvider;
use crate::error::{AgentError, Result, Retryable};
use crate::lifecycle::AgentLifecycle;
use crate::message_ext::AgentMessageExt;
use crate::prompt::compact;
use crate::prompt::system_prompt_builder;
use crate::skill::SharedSkillRuntime;
use crate::storage::{AgentSnapshot, AgentStorage, PersistOp};
use crate::tools::task_runtime::{TaskStatus, TaskStore};
use crate::tools::{ToolRegistry, Toolset};
use agentik_sdk::model::sanitize::sanitize_messages;

// ── Compaction constants ───────────────────────────────────────────

/// Maximum characters per tool output when serializing for summarization.
const TOOL_OUTPUT_MAX_CHARS: usize = 2_000;
/// Default tokens to preserve in the "recent" tail during compaction.
pub const DEFAULT_KEEP_TOKENS: u64 = 8_000;
/// Maximum tokens of recent user messages to carry forward into compacted
/// history (ported from Codex `COMPACT_USER_MESSAGE_MAX_TOKENS`).
const COMPACT_USER_MESSAGE_MAX_TOKENS: u64 = 20_000;
/// Minimum tokens of recent tool output to protect from pruning.
const PRUNE_PROTECT_TOKENS: u64 = 40_000;
/// Only prune if at least this many tokens can be freed.
const PRUNE_MINIMUM_TOKENS: u64 = 20_000;
/// Chars per token heuristic (matching OpenCode's `Token.estimate()`).
const CHARS_PER_TOKEN: usize = 4;
const MAX_RETRY_BACKOFF_SECS: u64 = 30;

// ── Error types (moved from memory/error.rs) ───────────────────────

pub mod error {
    use agentik_sdk::AnthropicError;
    use thiserror::Error;

    pub type Result<T> = std::result::Result<T, Error>;

    #[derive(Debug, Error)]
    pub enum Error {
        #[error("failed to compact: {0}")]
        Compact(#[from] AnthropicError),

        #[error(
            "orphaned tool_result: no matching tool_use block with id '{tool_use_id}' was found \
             in any message — this usually means the tool_use message was dropped or the \
             tool_result arrived out of order"
        )]
        OrphanToolResult { tool_use_id: String },
    }
}

// ─────────────────────────── AgentShared ───────────────────────────

/// Stable resources shared across all sessions of one agent.
///
/// Built once by [`AgentBuilder`](crate::agent_builder::AgentBuilder),
/// frozen into `Arc`, and cloned into every [`Session`]. Fields that need
/// runtime mutation use interior mutability (`ArcSwap`, etc.).
pub(crate) struct AgentShared {
    pub id: Uuid,
    pub path: agentik_types::AgentPath,
    pub config_json: serde_json::Value,
    pub model: Arc<ArcSwapOption<Model>>,
    pub config: AgentConfig,
    pub storage: Option<Arc<dyn AgentStorage>>,
    pub context_provider: Option<Arc<dyn ContextProvider>>,
    pub system_prompt_section: Option<String>,
    pub system_prompt_identity: Option<String>,
    pub(crate) memory: Option<Arc<crate::memory::MemoryBackend>>,
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
    /// `persist_tx`.
    pub persist_tx: std::sync::OnceLock<UnboundedSender<PersistOp>>,
    /// The agent's persistent task plan — a first-class citizen that lives
    /// as long as the agent does. Updated via the `update_plan` tool.
    /// Uses `Arc<ArcSwap>` for lock-free reads and sharing with the tool.
    pub plan: Arc<ArcSwap<AgentPlan>>,
}

impl AgentShared {
    /// The agent's short name (last path segment).
    pub fn name(&self) -> &str {
        self.path.name()
    }

    /// The full hierarchical agent path.
    pub fn path(&self) -> &agentik_types::AgentPath {
        &self.path
    }

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

    /// Build a minimal `AgentShared` suitable for unit-testing Session /
    /// add_message logic. No model, storage, persistence, or context provider
    /// is wired up.
    #[cfg(test)]
    pub(crate) fn new_for_tests() -> Arc<Self> {
        Arc::new(Self {
            id: Uuid::new_v4(),
            path: agentik_types::AgentPath::root(),
            config_json: serde_json::json!({}),
            model: Arc::new(ArcSwapOption::empty()),
            config: AgentConfig::default(),
            storage: None,
            context_provider: None,
            system_prompt_section: None,
            system_prompt_identity: None,
            memory: None,
            skill_runtime: None,
            tool_registry: Arc::new(ToolRegistry::new()),
            tasks: Arc::new(tokio::sync::RwLock::new(
                crate::tools::task_runtime::TaskStore::new(),
            )),
            event_tx: ArcSwapOption::empty(),
            persist_tx: std::sync::OnceLock::new(),
            plan: Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                agentik_types::AgentPlan::new(),
            ))),
        })
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

// ─────────────────────────── SessionState ───────────────────────────

/// Serializable conversation state — the snapshot payload.
///
/// Replaces the former `Memory` struct. Stores exactly what is needed to
/// reconstruct a session's conversation: the live messages, any compaction
/// summary for this session, and ancestor summaries from prior compactions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    pub messages: Vec<Message>,
    pub summary: Option<String>,
    /// Summaries from compaction ancestors, oldest first.
    /// Copied at compaction time so `render_context` is self-contained.
    #[serde(default)]
    pub ancestor_summaries: Vec<String>,
}

// ─────────────────────────── Session ───────────────────────────

/// One conversation within an agent. Owns the conversation data (formerly
/// Memory), lifecycle, toolset, and cancellation token.
pub struct Session {
    pub id: Uuid,
    pub title: Option<String>,
    pub created_at: i64,
    pub last_active: i64,

    // ── Conversation content (formerly Memory/MemoryItem) ──
    pub messages: Vec<Message>,
    pub summary: Option<String>,
    /// Summaries from compaction ancestors, oldest first.
    pub ancestor_summaries: Vec<String>,

    // ── Runtime state ──
    pub persist_tx: Option<UnboundedSender<PersistOp>>,
    pub lifecycle: AgentLifecycle,
    pub toolset: Toolset,
    pub token_budget: TokenBudget,
    pub cancel_token: CancellationToken,

    /// Active `wait_task` watchers keyed by task seq. Each value is the
    /// child cancel token for the spawned watcher. Before spawning a new
    /// watcher for the same task, the old one is cancelled to prevent
    /// duplicate message injection.
    pub(crate) active_wait_watchers: HashMap<u64, CancellationToken>,

    /// System prompt set by `build_context` and consumed by `request`.
    /// Kept separate from `messages` so the sanitizer never sees it
    /// and tool_use/tool_result indices are not shifted by coalescing.
    pub(crate) pending_system_prompt: Option<String>,
    /// Identity of the currently open conversation turn. A turn remains open
    /// across a `Waiting` pause (for example `wait_task`) and closes only on
    /// completion, interruption, or failure.
    pub(crate) active_turn_id: Option<Uuid>,
    pub(crate) active_delegation_id: Option<Uuid>,

    /// Back-reference to shared agent resources.
    pub(crate) shared: Arc<AgentShared>,
}

impl Session {
    /// Build a Session wired to a freshly-constructed `AgentShared`. Used
    /// only by unit tests that exercise `remember` / `add_message` without a
    /// full agent bootstrap.
    #[cfg(test)]
    pub(crate) fn new_for_tests(shared: Arc<AgentShared>, _path: agentik_types::AgentPath) -> Self {
        Self::new(Uuid::new_v4(), shared)
    }

    /// Create a new empty session.
    pub(crate) fn new(id: Uuid, shared: Arc<AgentShared>) -> Self {
        let toolset = Toolset::from_registry_with_tasks(
            shared.tool_registry.clone(),
            shared.tasks.clone(),
            shared.event_tx(),
        );
        let now = chrono::Utc::now().timestamp_millis();
        let persist_tx = shared.persist_tx.get().cloned();
        Self {
            id,
            title: None,
            created_at: now,
            last_active: now,
            messages: Vec::new(),
            summary: None,
            ancestor_summaries: Vec::new(),
            persist_tx,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token: CancellationToken::new(),
            active_wait_watchers: HashMap::new(),
            pending_system_prompt: None,
            active_turn_id: None,
            active_delegation_id: None,
            shared,
        }
    }

    /// Create a session pre-loaded with conversation state (restore path).
    pub(crate) fn new_with_state(
        id: Uuid,
        shared: Arc<AgentShared>,
        state: SessionState,
        cancel_token: CancellationToken,
    ) -> Self {
        let toolset = Toolset::from_registry_with_tasks(
            shared.tool_registry.clone(),
            shared.tasks.clone(),
            shared.event_tx(),
        );
        let now = chrono::Utc::now().timestamp_millis();
        let persist_tx = shared.persist_tx.get().cloned();
        Self {
            id,
            title: None,
            created_at: now,
            last_active: now,
            messages: state.messages,
            summary: state.summary,
            ancestor_summaries: state.ancestor_summaries,
            persist_tx,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token,
            active_wait_watchers: HashMap::new(),
            pending_system_prompt: None,
            active_turn_id: None,
            active_delegation_id: None,
            shared,
        }
    }

    /// Fork a new session from an existing one, deep-cloning its state.
    pub(crate) fn fork_from(parent: &Session, new_id: Uuid, shared: Arc<AgentShared>) -> Self {
        let toolset = Toolset::from_registry_with_tasks(
            shared.tool_registry.clone(),
            shared.tasks.clone(),
            shared.event_tx(),
        );
        let now = chrono::Utc::now().timestamp_millis();
        let persist_tx = shared.persist_tx.get().cloned();
        Self {
            id: new_id,
            title: Some(format!(
                "Fork of {}",
                parent.title.as_deref().unwrap_or("session")
            )),
            created_at: now,
            last_active: now,
            messages: parent.messages.clone(),
            summary: parent.summary.clone(),
            ancestor_summaries: parent.ancestor_summaries.clone(),
            persist_tx,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token: CancellationToken::new(),
            active_wait_watchers: HashMap::new(),
            pending_system_prompt: None,
            active_turn_id: None,
            active_delegation_id: None,
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

    /// Transition the lifecycle to `status`, emitting
    /// `AgentEvent::LifecycleChanged` if the status actually changed.
    pub(crate) fn set_lifecycle(&mut self, status: agentik_types::AgentLifecycleStatus) {
        if self.lifecycle.set_status(status) {
            self.shared.send_event(AgentEvent::LifecycleChanged(status));
        }
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
            state: SessionState {
                messages: self.messages.clone(),
                summary: self.summary.clone(),
                ancestor_summaries: self.ancestor_summaries.clone(),
            },
            session_id: Some(self.id),
        }
    }

    pub async fn persist_snapshot(&self) {
        let snapshot = self.snapshot();
        if let Some(storage) = &self.shared.storage {
            let _ = storage.as_ref().create_snapshot(snapshot).await;
        }
    }

    pub fn inject_message(
        &mut self,
        user_content: Vec<ContentBlock>,
        from_user: bool,
    ) -> Result<()> {
        // Emit a display event only for externally-sourced messages
        // (delegate_to / send_message / background-task notice). User-typed
        // messages are already shown locally by the TUI before being sent,
        // so emitting here would cause a duplicate display.
        if !from_user {
            let preview: String = user_content
                .iter()
                .filter_map(|cb| match cb {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !preview.is_empty() {
                self.shared.send_event(AgentEvent::MessageInjected(preview));
            }
        }

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
        self.remember(message)?;
        Ok(())
    }

    pub fn set_cancel_token(&mut self, token: CancellationToken) {
        self.cancel_token = token;
    }

    // ── Message management (formerly on Memory) ─────────────

    /// Remember a new message: append to `messages` (handling tool-result
    /// adjacency) and push to the WAL if persistence is active.
    pub fn remember(&mut self, message: Message) -> Result<()> {
        let for_persist = if self.persist_tx.is_some() {
            Some(message.clone())
        } else {
            None
        };

        self.add_message(message)?;

        if let (Some(tx), Some(msg)) = (&self.persist_tx, for_persist) {
            let _ = tx.send(PersistOp::AppendMessage {
                session_id: self.id,
                message: msg,
            });
        }

        Ok(())
    }

    /// Add a message to the conversation, ensuring tool_result blocks are
    /// placed adjacent to their corresponding tool_use blocks.
    ///
    /// Duplicate `tool_result` blocks for the same `tool_use_id` are
    /// collapsed at insert time — the API rejects multiple tool_results for
    /// the same id, and silently letting them in risks a 400 once the
    /// request finally goes out. Keeping the *first* occurrence (the one
    /// already in the conversation) is safer than keeping the new one,
    /// because the existing block already matches what the conversation
    /// expects the model to have seen.
    fn add_message(&mut self, msg: Message) -> Result<()> {
        let mut tool_results: Vec<(String, Option<String>, Option<bool>)> = Vec::new();
        let mut others_content_blocks: Vec<ContentBlock> = Vec::new();
        // Filter out tool_results
        for cb in &msg.content {
            match cb {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => tool_results.push((tool_use_id.clone(), content.clone(), *is_error)),
                _ => others_content_blocks.push(cb.clone()),
            }
        }

        if !others_content_blocks.is_empty() {
            // De-dupe consecutive identical user text messages. Background
            // watchers (wait_task timeout) can inject the same message
            // multiple times if the LLM calls wait_task in a loop. Without
            // this guard, each copy is persisted and wastes context.
            if msg.role == Role::User {
                if let Some(last) = self.messages.last() {
                    if last.role == Role::User && last.content == others_content_blocks {
                        tracing::debug!(
                            "dropping duplicate consecutive user message ({} bytes)",
                            others_content_blocks
                                .iter()
                                .map(|cb| match cb {
                                    ContentBlock::Text { text } => text.len(),
                                    _ => 0,
                                })
                                .sum::<usize>()
                        );
                        return Ok(());
                    }
                }
            }
            let mut other_msg = msg.clone();
            other_msg.content = others_content_blocks;
            self.messages.push(other_msg);
        }

        for (tool_use_id, content, is_error) in tool_results {
            // De-dupe: if a tool_result for this id already exists anywhere
            // in the conversation, skip the new one. (The sanitisers in
            // `agentik_sdk::model::sanitize` would also catch this, but
            // stopping it at the entry point avoids needless churn and
            // prevents the duplicate from ever reaching the WAL.)
            if self.has_tool_result(&tool_use_id) {
                tracing::debug!(
                    tool_use_id = %tool_use_id,
                    "dropping duplicate tool_result at insert time"
                );
                continue;
            }

            let tc_msg_index =
                self.get_tooluse_msg_index(&tool_use_id)
                    .ok_or(error::Error::OrphanToolResult {
                        tool_use_id: tool_use_id.clone(),
                    })?;

            if tc_msg_index + 1 < self.messages.len() {
                // Need to move tool_result to the next message of tool_use
                if matches!(self.messages[tc_msg_index + 1].role, Role::User) {
                    self.messages[tc_msg_index + 1]
                        .content
                        .push(ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        });
                } else {
                    // Slot at tc_msg_index + 1 is non-User (e.g. another
                    // assistant turn, or a checkpoint summary message).
                    // Insert a fresh User message carrying only the
                    // tool_result so the Anthropic adjacency invariant
                    // (every tool_use followed immediately by a user
                    // message with the matching tool_result) is
                    // preserved at insertion. The previous error
                    // (`UnexpectedMessageLayout`) was the source of
                    // messages.1118 violations.
                    let mut tool_res_msg = msg.clone();
                    tool_res_msg.content = vec![ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    }];
                    self.messages.insert(tc_msg_index + 1, tool_res_msg);
                }
            } else {
                let mut tool_res_msg = msg.clone();
                tool_res_msg.content = vec![ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                }];
                self.messages.push(tool_res_msg);
            }
        }
        Ok(())
    }

    /// True when any message in the conversation already carries a
    /// `tool_result` block for `tool_use_id`.
    fn has_tool_result(&self, tool_use_id: &str) -> bool {
        self.messages.iter().any(|m| {
            m.content.iter().any(|c| {
                matches!(c, ContentBlock::ToolResult { tool_use_id: id, .. } if id == tool_use_id)
            })
        })
    }

    /// Get index of message with a tool_use block matching `tool_call_id`.
    fn get_tooluse_msg_index(&self, tool_call_id: &str) -> Option<usize> {
        self.messages.iter().position(|m| {
            m.content
                .iter()
                .any(|c| matches!(c, ContentBlock::ToolUse { id, .. } if id == tool_call_id))
        })
    }

    /// Render the full conversation context for the LLM.
    ///
    /// - Historical compaction summaries are injected as
    ///   `<conversation-checkpoint>` user messages BEFORE current messages.
    /// - Old tool outputs exceeding the protection window are pruned.
    pub fn render_context(&self) -> Result<Vec<Message>> {
        let mut result = Vec::new();

        // 1. Inject summaries from all compaction ancestors
        for summary in &self.ancestor_summaries {
            let formatted = format_checkpoint_message(summary);
            result.push(Message::user(formatted));
        }

        // 2. Append the current messages
        let mut messages = self.messages.clone();
        prune_old_tool_outputs(&mut messages, PRUNE_PROTECT_TOKENS);
        result.extend(messages);

        Ok(result)
    }

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
    async fn do_manual_compact(&mut self) {
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

        self.shared.send_event(AgentEvent::Compact {
            event: CompactEvent::CompactFinish { ts: Utc::now() },
        });
        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Idle);
    }

    // ── Pause / Resume ────────────────────────────────────

    /// Pause the session: lifecycle → Idle, persist snapshot, end WAL session.
    pub async fn pause(&mut self) {
        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Idle);
        self.persist_snapshot().await;
        if let Some(storage) = &self.shared.storage {
            let _ = storage.end_session(self.id).await;
        }
    }

    /// Run the Anthropic-invariant sanitizer over `self.messages` and, if
    /// any repair was performed, replace the in-memory conversation with
    /// the sanitized version and emit `PersistOp::ReplaceSessionState` so
    /// the WAL reflects the cleaned shape.
    ///
    /// Without this, `Model::request` would silently run `sanitize_messages`
    /// at the API boundary and throw the patched result away — meaning
    /// every subsequent turn would re-patch the same broken shape and any
    /// snapshot/restore would resurrect the broken state. This method
    /// makes the repair *durable*.
    ///
    /// Cheap when nothing needs fixing: `sanitize_messages` returns the
    /// input `Vec` untouched (no allocation), so we only enter the WAL
    /// path when at least one rule fired.
    pub fn sanitize_and_persist(&mut self) {
        // Compare lengths before/after — same length means no repair was
        // needed (sanitize_messages either returns the input unchanged or
        // rebuilds a new Vec with potentially different length).
        let before_len = self.messages.len();
        let original_ptr = self.messages.as_ptr();
        let sanitized = sanitize_messages(std::mem::take(&mut self.messages));
        let same_alloc = sanitized.as_ptr() == original_ptr && sanitized.len() == before_len;
        self.messages = sanitized;
        if same_alloc {
            return;
        }
        let after_len = self.messages.len();
        tracing::info!(
            messages_before = before_len,
            messages_after = after_len,
            "session messages sanitized (Anthropic-invariant repair); persisting"
        );

        if let Some(tx) = &self.persist_tx {
            let state = SessionState {
                messages: self.messages.clone(),
                summary: self.summary.clone(),
                ancestor_summaries: self.ancestor_summaries.clone(),
            };
            let _ = tx.send(PersistOp::ReplaceSessionState {
                agent_id: self.shared.id,
                session_id: self.id,
                state,
            });
        }
    }

    /// Resume the session: open WAL session (if not already open), lifecycle → Idle.
    pub async fn resume(&mut self) {
        // Start a WAL session if we don't have persist_tx wired yet.
        if self.persist_tx.is_none() {
            if let Some(tx) = self.shared.persist_tx.get() {
                self.persist_tx = Some(tx.clone());
            }
        }
        if let Some(storage) = &self.shared.storage {
            let _ = storage.start_session(self.shared.id, self.id).await;
            let _ = storage.touch_agent(self.shared.id).await;
        }
        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Idle);
        self.last_active = chrono::Utc::now().timestamp_millis();
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

    // ── Session loop ──────────────────────────────────────

    /// Open a turn, reusing an open turn after a background-task wait.
    pub(crate) fn begin_turn(&mut self, delegation_id: Option<Uuid>) {
        if self.active_turn_id.is_none() {
            self.active_turn_id = Some(Uuid::new_v4());
            self.active_delegation_id = delegation_id;
        } else if delegation_id.is_some() {
            self.active_delegation_id = delegation_id;
        }

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
        self.shared.send_event(AgentEvent::TurnCompleted {
            turn_id,
            session_id: self.id,
            delegation_id,
            status,
        });
    }

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

    /// After a mid-workflow cancellation, patch orphaned tool_use blocks.
    async fn patch_orphaned_tool_use(&mut self) {
        let Some(last_msg) = self.messages.last() else {
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
        let _ = self.remember(stub);
    }

    /// Core agent workflow: build context → request API → execute tools → remember.
    async fn agent_workflow(
        &mut self,
        internal_event_tx: &UnboundedSender<InternalEvent>,
        retry_feedback: Option<String>,
    ) -> Result<()> {
        if let Some(feedback) = retry_feedback {
            self.inject_message(vec![ContentBlock::Text { text: feedback }], false)
                .unwrap();
        }

        self.poll_context_provider().await;

        let context = self.build_context().await?;
        let allowed = self.current_allowed_tools().await;

        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Requesting);
        self.shared.send_event(AgentEvent::Requesting);
        let response_message = self
            .request_with_retries(context, allowed.as_deref())
            .await?;

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

        self.token_budget.record_api_usage(&last_usage);

        self.remember(response_message.clone())?;
        self.token_budget.add_pending_message(&response_message);

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
                    .unwrap_or(120);

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
                tracing::debug!(
                    context_length,
                    current_total = self.token_budget.current_total(),
                    "mid-turn context pressure detected, compacting before next iteration"
                );
                self.set_lifecycle(agentik_types::AgentLifecycleStatus::Compacting);
                self.shared.send_event(AgentEvent::Compact {
                    event: CompactEvent::CompactStart { ts: Utc::now() },
                });
                self.compact(model.as_ref()).await?;
                self.shared.send_event(AgentEvent::Compact {
                    event: CompactEvent::CompactFinish { ts: Utc::now() },
                });
                self.set_lifecycle(agentik_types::AgentLifecycleStatus::Requesting);
            }
        }

        Ok(())
    }

    /// Retry retryable API failures without replaying tool side effects.
    ///
    /// The retry boundary is intentionally inside the workflow's model-request
    /// phase. Retrying the whole workflow after a later failure could execute
    /// tools twice, while retrying here only repeats the LLM request.
    async fn request_with_retries(
        &mut self,
        mut context: Vec<Message>,
        allowed: Option<&[String]>,
    ) -> Result<Message> {
        let max_retries: u32 = self
            .shared
            .config
            .max_retries
            .try_into()
            .unwrap_or(u32::MAX);
        let mut attempts_used = 0u32;

        loop {
            match self.request(context, allowed).await {
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
                    context = self.build_context().await?;
                }
                Err(e) => return Err(e),
            }
        }
    }

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
        cancel: tokio_util::sync::CancellationToken,
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
                 It is still running. You can check its status with `view_task_status` or \
                 wait again with `wait_task`."
            )
        };

        let _ = tx.send(InternalEvent::MessageInject {
            content: vec![ContentBlock::Text { text: message }],
            from_user: false,
            delegation_id: None,
        });
    }

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

        let identity = self
            .shared
            .system_prompt_identity
            .as_deref()
            .unwrap_or("You are a helpful assistant.");
        builder = builder.with_identity(format!(
            "{identity}\n\n\
             Your agent path is `{path}`. Your short name is **{name}**. \
             Other agents can address you by your short name or full path. \
             When spawning child agents, their path is derived from yours. \
             Do not delegate tasks to yourself — use `list_agents` or `route_task` \
             to find a *different* agent suited for the task.",
            path = self.shared.path.as_str(),
            name = self.shared.name(),
        ));

        if let Some(ref extra) = self.shared.system_prompt_section {
            builder = builder.with_extra_section(extra);
        }

        if let Some(memory) = &self.shared.memory {
            if let Some(section) = memory.prompt_section().await {
                builder = builder.with_extra_section(section);
            }
        }

        if let Some(rt) = &self.shared.skill_runtime {
            let section = rt.lock().await.current_prompt_section();
            if !section.is_empty() {
                builder = builder.with_extra_section(section);
            }
        }

        let plan = self.shared.plan_snapshot();
        if !plan.is_empty() {
            builder = builder.with_extra_section(render_plan_prompt_section(&plan));
        }

        let system_prompt = builder.parse();
        let context_messages = self.render_context()?.to_vec();

        // Store system prompt on the session so request() can pass it
        // via params.system instead of mixing it into messages.
        self.pending_system_prompt = Some(system_prompt);

        Ok(context_messages)
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

        let conversation_msgs = self.render_context()?;
        if self
            .token_budget
            .should_compact(&conversation_msgs, model.model_info.context_length)
        {
            tracing::debug!(
                context_length = model.model_info.context_length,
                current_total = self.token_budget.current_total(),
                "pre-request context pressure detected (≥90% of context window), compacting"
            );
            self.set_lifecycle(agentik_types::AgentLifecycleStatus::Compacting);
            self.shared.send_event(AgentEvent::Compact {
                event: CompactEvent::CompactStart { ts: Utc::now() },
            });
            let compacted = self.compact(model.as_ref()).await?;
            self.shared.send_event(AgentEvent::Compact {
                event: CompactEvent::CompactFinish { ts: Utc::now() },
            });
            self.set_lifecycle(agentik_types::AgentLifecycleStatus::Requesting);
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

        // Repair any Anthropic-invariant violations before sending and
        // persist the repaired shape so future turns don't re-patch.
        self.sanitize_and_persist();
        // sanitize_and_persist may have modified self.messages (dedup,
        // coalesce, adjacency repair). Rebuild context from the
        // repaired messages so the API receives the clean shape.
        context = self.build_context().await?;

        // Final defensive sanitize on the actual context (without
        // system prompt — it's stored separately now, so there's no
        // same-role coalescing risk).
        context = sanitize_messages(context);

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

fn is_retryable_api_request(error: &AgentError) -> bool {
    matches!(error, AgentError::ApiRequestError(e) if e.is_retryable())
}

// Suppress unused import warning for ArcSwap (used in AgentShared.event_tx type).
#[allow(unused_imports)]
use ArcSwap as _ArcSwap;

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
fn prune_old_tool_outputs(messages: &mut [Message], protect_tokens: u64) {
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
fn format_checkpoint_message(summary: &str) -> String {
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
    use crate::message_ext::AgentMessageExt;
    use agentik_types::messages::ContentBlock;

    /// Regression for `add_message` collapsing duplicate `tool_result` blocks
    /// when the unconditional `.push()` in the next-message branch would
    /// otherwise leave two `tool_result` blocks for the same `tool_use_id`
    /// in the conversation. Without this guard the next request would 400
    /// with `each tool_use must have a single result`.
    #[test]
    fn add_message_drops_duplicate_tool_result_on_remember() {
        let mut session = make_test_session();
        session
            .remember(Message::user("hello"))
            .expect("first remember ok");
        session
            .remember(Message::assistant_tool_use(
                "call_001",
                "bash",
                serde_json::json!({}),
            ))
            .expect("tool_use remember ok");

        // First tool_result — this is the legitimate one.
        session
            .remember(Message::tool_result("call_001", "first", false))
            .expect("first tool_result ok");

        // Snapshot how many tool_result blocks for call_001 exist now.
        let count_before = count_tool_results(&session.messages, "call_001");
        assert_eq!(count_before, 1);

        // Second remember with the same tool_use_id — must be dropped.
        session
            .remember(Message::tool_result(
                "call_001",
                "second (duplicate)",
                false,
            ))
            .expect("duplicate tool_result should not error");

        let count_after = count_tool_results(&session.messages, "call_001");
        assert_eq!(
            count_after, 1,
            "duplicate tool_result must be collapsed at insert time"
        );
        // And the *first* content should be preserved.
        let first_body = session
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .find_map(|c| match c {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } if tool_use_id == "call_001" => Some(content.clone()),
                _ => None,
            })
            .expect("at least one tool_result remains");
        assert_eq!(first_body.as_deref(), Some("first"));
    }

    /// Two distinct `tool_use_id`s must both be retained — the dedup is
    /// per-id, not "drop everything past the first".
    #[test]
    fn add_message_keeps_distinct_tool_results() {
        let mut session = make_test_session();
        session.remember(Message::user("hi")).unwrap();
        session
            .remember(Message {
                id: "a".into(),
                type_: "message".into(),
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "call_a".into(),
                        name: "bash".into(),
                        input: serde_json::json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "call_b".into(),
                        name: "bash".into(),
                        input: serde_json::json!({}),
                    },
                ],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            })
            .unwrap();
        session
            .remember(Message::tool_result("call_a", "alpha", false))
            .unwrap();
        session
            .remember(Message::tool_result("call_b", "beta", false))
            .unwrap();
        let a = count_tool_results(&session.messages, "call_a");
        let b = count_tool_results(&session.messages, "call_b");
        assert_eq!(a, 1);
        assert_eq!(b, 1);
    }

    fn count_tool_results(msgs: &[Message], tool_use_id: &str) -> usize {
        msgs.iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, ContentBlock::ToolResult { tool_use_id: id, .. } if id == tool_use_id))
            .count()
    }

    /// Construct a Session with no model / no storage / no events wired.
    /// Sufficient for testing `remember` / `add_message` paths in isolation.
    fn make_test_session() -> Session {
        let shared = AgentShared::new_for_tests();
        Session::new_for_tests(shared, agentik_types::AgentPath::root())
    }

    /// When the slot immediately after a `tool_use` is non-User (e.g.
    /// another assistant turn or a checkpoint user message pushed in
    /// between the call and its result), `add_message` must insert a
    /// fresh User message carrying the `tool_result` so Anthropic's
    /// adjacency invariant holds at the source. Previously this returned
    /// `UnexpectedMessageLayout`, which surfaced as the messages.1118
    /// 400 from the API.
    #[test]
    fn add_message_inserts_user_slot_when_next_is_non_user() {
        let mut session = make_test_session();
        session.remember(Message::user("hi")).unwrap();
        session
            .remember(Message {
                id: "a".into(),
                type_: "message".into(),
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_X".into(),
                    name: "t".into(),
                    input: serde_json::json!({}),
                }],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            })
            .unwrap();
        // Push a non-user message between the tool_use and its result.
        session.messages.push(Message {
            id: "u_intermediate".into(),
            type_: "message".into(),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "checkpoint summary".into(),
            }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        });
        session
            .remember(Message::tool_result("call_X", "alpha", false))
            .unwrap();
        // Find the assistant tool_use and assert the immediately next
        // message is a User carrying tool_result(call_X).
        let asst_idx = session
            .messages
            .iter()
            .position(|m| m.has_tool_use())
            .expect("assistant with tool_use");
        let next = &session.messages[asst_idx + 1];
        assert!(
            matches!(next.role, Role::User),
            "next message must be User (inserted for adjacency)"
        );
        let has_result = next.content.iter().any(|c| {
            matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_X")
        });
        assert!(
            has_result,
            "adjacent user message must carry tool_result(call_X)"
        );
        let count = session
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_X"))
            .count();
        assert_eq!(
            count, 1,
            "tool_result must be inserted exactly once (no duplicates)"
        );
    }

    /// `sanitize_and_persist` must repair a misadjacent tool_result and
    /// leave the conversation well-formed. Without it, every LLM
    /// request would re-patch the same broken state and any snapshot
    /// would resurrect the broken shape.
    #[test]
    fn sanitize_and_persist_repairs_misadjacent_tool_result() {
        let mut session = make_test_session();
        // Build: [user, assistant{tool_use}, user{retry text},
        //        user{tool_result}] — the tool_result is one user
        // message too late.
        session.remember(Message::user("hi")).unwrap();
        session
            .remember(Message {
                id: "a".into(),
                type_: "message".into(),
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_X".into(),
                    name: "t".into(),
                    input: serde_json::json!({}),
                }],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            })
            .unwrap();
        session
            .remember(Message {
                id: "u_retry".into(),
                type_: "message".into(),
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "retry please".into(),
                }],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            })
            .unwrap();
        session
            .remember(Message::tool_result("call_X", "ok", false))
            .unwrap();

        let before_len = session.messages.len();
        session.sanitize_and_persist();
        let after_len = session.messages.len();

        // Adjacency: tool_use(call_X) must be followed immediately by a
        // user message containing tool_result(call_X).
        let asst_idx = session
            .messages
            .iter()
            .position(|m| m.has_tool_use())
            .expect("tool_use assistant remains");
        let next = &session.messages[asst_idx + 1];
        assert!(matches!(next.role, Role::User));
        let has_adjacent = next.content.iter().any(|c| {
            matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_X")
        });
        assert!(
            has_adjacent,
            "after sanitize_and_persist, tool_result(call_X) must sit in the adjacent user message"
        );
        // The repair moved blocks between messages; the count may change
        // (dedup + relocate). What matters is that exactly one
        // tool_result for call_X survives.
        let total = session
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_X"))
            .count();
        assert_eq!(total, 1, "exactly one tool_result for call_X must remain");
        // Length may have changed (one synthetic user message inserted
        // / one empty after-retract dropped).
        let _ = (before_len, after_len);
    }

    /// `sanitize_and_persist` must be a no-op when the conversation is
    /// already well-formed (fast-path: same Vec returned by
    /// `sanitize_messages`).
    #[test]
    fn sanitize_and_persist_is_noop_on_clean_conversation() {
        let mut session = make_test_session();
        session.remember(Message::user("hi")).unwrap();
        session.remember(Message::assistant_text("hello")).unwrap();
        session.remember(Message::user("how are you?")).unwrap();
        let before = session.messages.clone();
        session.sanitize_and_persist();
        // Clean input: no change in messages, no synthetic inserts.
        assert_eq!(session.messages.len(), before.len());
        for (a, b) in session.messages.iter().zip(before.iter()) {
            assert_eq!(a.role, b.role);
            assert_eq!(a.content.len(), b.content.len());
        }
    }

    /// Consecutive identical user text messages must be collapsed to a
    /// single copy. Background `wait_task` watchers can inject the same
    /// timeout message multiple times when the LLM calls wait_task in a
    /// loop — without this guard each copy is persisted and wastes context.
    #[test]
    fn add_message_drops_consecutive_duplicate_user_text() {
        let mut session = make_test_session();
        let msg_text = "Background task 'download' (#50) did not complete \
                         within 120 seconds. It is still running.";
        session.remember(Message::user(msg_text)).unwrap();
        let count_after_first = session.messages.len();

        // Second identical message — must be dropped.
        session.remember(Message::user(msg_text)).unwrap();
        assert_eq!(
            session.messages.len(),
            count_after_first,
            "duplicate consecutive user message must be dropped"
        );

        // Third identical message — still dropped.
        session.remember(Message::user(msg_text)).unwrap();
        assert_eq!(
            session.messages.len(),
            count_after_first,
            "third identical user message must also be dropped"
        );
    }

    /// Non-identical consecutive user messages must be kept. Only exact
    /// duplicates are collapsed.
    #[test]
    fn add_message_keeps_distinct_consecutive_user_text() {
        let mut session = make_test_session();
        session.remember(Message::user("first message")).unwrap();
        session.remember(Message::user("second message")).unwrap();
        assert_eq!(
            session.messages.len(),
            2,
            "distinct consecutive user messages must both be kept"
        );
    }

    /// After a different message (e.g. assistant reply) interleaves, the
    /// same text can appear again — the dedup is strictly consecutive.
    #[test]
    fn add_message_allows_same_text_after_interleaving() {
        let mut session = make_test_session();
        session.remember(Message::user("hello")).unwrap();
        session
            .remember(Message::assistant_text("hi there"))
            .unwrap();
        // Now "hello" again is fine — not consecutive duplicate.
        session.remember(Message::user("hello")).unwrap();
        assert_eq!(
            session.messages.len(),
            3,
            "same text after an interleaved message is not a duplicate"
        );
    }
}
