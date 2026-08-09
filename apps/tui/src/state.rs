use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock};

use crate::widgets::input_area::{InputArea, InputState};
use agentik_sdk::model::Model;
use agentik_sdk::types::{AgentEvent, CompactEvent};
use arc_swap::ArcSwapOption;
use ratatui::text::Line;

// ── Tool task tracking ─────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolTaskStatus {
    Running,
    Done { ok: bool },
}

#[derive(Debug, Clone)]
pub struct ToolTaskInfo {
    pub seq: u64,
    pub name: String,
    pub status: ToolTaskStatus,
}

// ── Chat data model ─────────────────────────────────────

/// Token usage for a single LLM turn, attached to the assistant message it
/// produced. `input_tokens` is `Option` because the streaming protocol only
/// reports it on the final delta of a turn.
#[derive(Debug, Clone, Copy)]
pub struct TurnUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
}

/// Snapshot of the agent's current task plan, maintained by `apply_event`
/// when `AgentEvent::PlanUpdate` arrives.
#[derive(Debug, Clone, Default)]
pub struct PlanState {
    /// The latest plan snapshot. Empty when no plan has been set.
    pub steps: Vec<agentik_types::PlanStep>,
    /// Optional explanation from the last update.
    pub explanation: Option<String>,
    /// Revision counter from the agent.
    pub revision: u64,
}

impl PlanState {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// `(completed, total)` or `None` if the plan is empty.
    pub fn progress(&self) -> Option<(usize, usize)> {
        let total = self.steps.len();
        if total == 0 {
            return None;
        }
        let done = self
            .steps
            .iter()
            .filter(|s| s.status == agentik_types::StepStatus::Completed)
            .count();
        Some((done, total))
    }
}

#[derive(Debug, Clone)]
pub enum ChatLine {
    User(String),
    Assistant {
        text: String,
        usage: Option<TurnUsage>,
    },
    Thinking(String),
    ToolCall {
        name: String,
        input: String,
    },
    ToolResult {
        ok: bool,
        content: String,
    },
    /// Tool is running in the background (sync phase expired).
    ToolBackground {
        seq: u64,
        name: String,
    },
    Error(String),
    /// A retryable error occurred; the agent is backing off before retrying.
    RetryableError {
        message: String,
        attempt: u32,
        max_retries: u32,
    },
    Separator,
}

/// Unified agent lifecycle status — re-exported from `agentik-types` so
/// the TUI shares a single source of truth with the core layer.
///
/// See [`agentik_types::AgentLifecycleStatus`] for the full state diagram.
pub use agentik_types::AgentLifecycleStatus as AgentStatus;

/// Modal state of the agent tab's input surface.
///
/// `Browse` scrolls the transcript; `Input` is the composing mode where
/// keystrokes insert text and Enter sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Browse,
    /// Insert mode — keystrokes insert text; Enter sends.
    Input,
}

/// Mutable state for the Agent tab.
pub struct AgentTabState {
    pub messages: Vec<ChatLine>,
    pub tool_tasks: Vec<ToolTaskInfo>,
    pub scroll_offset: usize,
    pub status: AgentStatus,
    pub input: InputArea,
    /// History of submitted prompts, newest at the back. Used by
    /// `Up`/`Down` to recall previous messages.
    pub input_history: VecDeque<String>,
    /// Maximum number of retained history entries. Oldest get evicted
    /// when capacity is reached.
    pub input_history_capacity: usize,
    /// Buffer snapshot saved when the user first presses `Up` so the
    /// in-progress edit is restored on `Down` past the oldest recall.
    pub input_draft: Option<String>,
    /// `None` = editing the user's draft (no recall in progress).
    /// `Some(idx)` = displaying `input_history[idx]`. The first non-`Up`/
    /// `Down` keystroke collapses back to draft mode.
    pub input_recall: Option<usize>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    /// Tokens consumed by the most recent completed turn. Reset at the start
    /// of each turn (on `Requesting`) and updated when the final UsageUpdate
    /// delta arrives. The status bar shows these instead of the cumulative
    /// totals so the user can see per-turn cost at a glance.
    pub latest_turn_input_tokens: u64,
    pub latest_turn_output_tokens: u64,
    pub latest_turn_cache_read_tokens: u64,
    pub latest_turn_cache_creation_tokens: u64,
    /// Total prompt tokens billed for the latest turn, i.e.
    /// `input_tokens + cache_creation + cache_read`. This is the value
    /// that counts against the model's context window, so it powers the
    /// context-window progress bar.
    pub latest_turn_context_used: u64,
    pub input_mode: InputMode,
    /// Messages the user typed while the agent was busy. Each entry is
    /// delivered to the agent when the current response cycle finishes
    /// (on `AgentEvent::Done` / `AgentEvent::Error`). The chat view
    /// already shows them as user messages (pushed at enqueue time) —
    /// the queue only tracks what still needs to reach the agent.
    pub pending_queue: VecDeque<String>,
    /// When true, `clamp_scroll` forces offset to the bottom each frame.
    pub auto_scroll: bool,
    /// True while an incremental Ctrl+R history search is in progress.
    pub in_history_search: bool,
    /// The current Ctrl+R search query (typed by the user).
    pub history_search_query: String,
    /// Indices into `input_history` that match `history_search_query`,
    /// newest-first. Recomputed whenever the query changes.
    pub history_search_matches: Vec<usize>,
    /// Selected position within `history_search_matches` (0 = newest match).
    pub history_search_selected: usize,
    /// Snapshot of the input buffer saved when Ctrl+R starts, restored on Esc.
    pub history_search_draft: Option<String>,
    /// Cached total rendered line count (updated every frame).
    pub content_line_count: usize,
    /// Per-message content version (parallel to `messages`). Bumped whenever
    /// the rendered output of that message would change (text append, usage
    /// update, etc.). The render cache compares these to decide which messages
    /// need re-rendering.
    pub msg_versions: Vec<u64>,
    /// Monotonic counter backing `msg_versions`; each push/mutation assigns the
    /// next value.
    msg_version_counter: u64,
    /// Per-message rendered-line cache (parallel to `messages`). Entry *i*
    /// holds the last rendered `Vec<Line>` for `messages[i]`.
    pub cached_msg_lines: Vec<Vec<Line<'static>>>,
    /// The `msg_versions[i]` value at which `cached_msg_lines[i]` was rendered.
    /// Zero means stale (never rendered).
    pub cached_msg_versions: Vec<u64>,
    /// Terminal width at which the per-message cache was built.
    pub cached_msg_width: u16,
    /// DisplaySettings.version at which cache was built; mismatch = invalidate.
    pub cached_display_version: u64,
    /// Index of the assistant line currently being streamed, so incoming
    /// `UsageUpdate` events can be attributed to the right turn. Reset on each
    /// `Requesting` (one per LLM call); `None` for tool-only turns.
    pub streaming_assistant: Option<usize>,
    /// Monotonic frame counter, incremented each render tick while the agent
    /// is active (Requesting / Streaming). Drives loading animations.
    pub frame: u64,
    pub compact_state: CompactState,
    /// Agent task plan (updated via `update_plan` tool events).
    pub plan: PlanState,
}

#[derive(Debug, Default)]
pub struct CompactState {
    pub is_compacting: bool,
}

impl Default for AgentTabState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            tool_tasks: Vec::new(),
            scroll_offset: 0,
            status: AgentStatus::Idle,
            input: InputArea::new(),
            input_history: VecDeque::new(),
            input_history_capacity: 200,
            input_draft: None,
            input_recall: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            latest_turn_input_tokens: 0,
            latest_turn_output_tokens: 0,
            latest_turn_cache_read_tokens: 0,
            latest_turn_cache_creation_tokens: 0,
            latest_turn_context_used: 0,
            input_mode: InputMode::Browse,
            pending_queue: VecDeque::new(),
            auto_scroll: true,
            in_history_search: false,
            history_search_query: String::new(),
            history_search_matches: Vec::new(),
            history_search_selected: 0,
            history_search_draft: None,
            content_line_count: 0,
            msg_versions: Vec::new(),
            msg_version_counter: 0,
            cached_msg_lines: Vec::new(),
            cached_msg_versions: Vec::new(),
            cached_msg_width: 0,
            cached_display_version: 0,
            streaming_assistant: None,
            frame: 0,
            compact_state: CompactState::default(),
            plan: PlanState::default(),
        }
    }
}

impl AgentTabState {
    /// Returns true when the user can type and send messages.
    pub fn can_send(&self) -> bool {
        self.status == AgentStatus::Idle && !self.input.is_empty()
    }

    /// Returns true when the composer has text that can be enqueued for
    /// later delivery (agent is busy). The text is taken from the input
    /// field; the queue is drained when the agent returns to idle.
    pub fn can_enqueue(&self) -> bool {
        !self.input.is_empty()
    }

    /// Number of messages waiting in the pending queue.
    pub fn pending_queue_len(&self) -> usize {
        self.pending_queue.len()
    }

    /// Push a message onto the pending queue (to be delivered when the
    /// agent goes idle).
    pub fn enqueue_pending(&mut self, text: String) {
        self.pending_queue.push_back(text);
    }

    /// Drain all pending messages, returning them in FIFO order.
    pub fn drain_pending_queue(&mut self) -> Vec<String> {
        self.pending_queue.drain(..).collect()
    }

    /// Take the current input text and clear the input field.
    pub fn take_input(&mut self) -> String {
        let text = self.input.value();
        self.input.clear();
        text
    }

    /// Push a user message and a separator after the previous assistant response.
    pub fn push_user_message(&mut self, text: String) {
        self.push_line(ChatLine::User(text));
    }

    // ── Per-message versioning helpers ──
    //
    // These keep `msg_versions` in lockstep with `messages` so the render
    // cache in `AgentLeaf` can skip re-rendering unchanged messages.

    /// Push a new chat line, assigning it a fresh content version.
    pub fn push_line(&mut self, line: ChatLine) {
        self.msg_version_counter = self.msg_version_counter.wrapping_add(1);
        self.messages.push(line);
        self.msg_versions.push(self.msg_version_counter);
    }

    /// Replace the entire message list in one shot, keeping `msg_versions`,
    /// `cached_msg_lines`, and `cached_msg_versions` in lockstep. Use this
    /// instead of assigning `messages` directly (e.g. when replaying history
    /// from storage).
    pub fn set_messages(&mut self, messages: Vec<ChatLine>) {
        let n = messages.len();
        self.msg_version_counter = self.msg_version_counter.wrapping_add(1);
        self.messages = messages;
        self.msg_versions = (0..n).map(|_| self.msg_version_counter).collect();
        self.cached_msg_lines = vec![Vec::new(); n];
        self.cached_msg_versions = vec![0; n];
        self.content_line_count = n;
    }

    /// Bump the content version of the last message (its content mutated in place).
    fn bump_last_version(&mut self) {
        self.msg_version_counter = self.msg_version_counter.wrapping_add(1);
        if let Some(v) = self.msg_versions.last_mut() {
            *v = self.msg_version_counter;
        }
    }

    /// Bump the content version of the message at `idx`.
    fn bump_version_at(&mut self, idx: usize) {
        self.msg_version_counter = self.msg_version_counter.wrapping_add(1);
        if let Some(v) = self.msg_versions.get_mut(idx) {
            *v = self.msg_version_counter;
        }
    }

    /// Re-enable auto-scroll so the next `clamp_scroll` call pins to the bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.auto_scroll = true;
    }

    /// Clamp scroll_offset based on content and viewport.
    /// When `auto_scroll` is true, forces offset to the maximum (bottom).
    pub fn clamp_scroll(&mut self, viewport_height: u16) {
        let total = self.content_line_count;
        let max_offset = total.saturating_sub(viewport_height as usize);
        if self.auto_scroll {
            self.scroll_offset = max_offset;
        } else {
            self.scroll_offset = self.scroll_offset.min(max_offset);
        }
    }

    /// Returns true when the viewport is showing the bottom of the content.
    pub fn is_at_bottom(&self, viewport_height: u16) -> bool {
        let max_offset = self
            .content_line_count
            .saturating_sub(viewport_height as usize);
        self.scroll_offset >= max_offset
    }
}

// ── Event → State mapping ──────────────────────────────

/// Apply an `AgentEvent` to `AgentTabState`, mutating the conversation view.
pub fn apply_event(state: &mut AgentTabState, event: AgentEvent) {
    match event {
        // ── Lifecycle status changes ──
        // The authoritative source for agent status. Emitted by the core
        // before the corresponding domain event (Requesting, Done, etc.).
        AgentEvent::LifecycleChanged(status) => {
            state.status = status;
        }
        AgentEvent::Requesting => {
            // A new LLM call begins — clear the streaming-assistant handle so
            // usage from this call isn't attributed to a prior turn's line.
            state.streaming_assistant = None;
            // NOTE: do NOT reset `latest_turn_*` here. Anthropic's streaming
            // protocol only emits the final input/cache counts on the last
            // UsageUpdate delta, so during a turn `input_tokens` is `None` and
            // any reset would leave the status bar blank. Keep the previous
            // turn's values visible until the new turn's final UsageUpdate
            // arrives (which will overwrite `latest_turn_*` in the UsageUpdate
            // branch below).
        }
        AgentEvent::TextDelta(text) => {
            // Append to last Assistant line, or create a new one
            let last_is_assistant = state
                .messages
                .last()
                .is_some_and(|l| matches!(l, ChatLine::Assistant { .. }));
            if last_is_assistant {
                if let Some(ChatLine::Assistant { text: s, .. }) = state.messages.last_mut() {
                    s.push_str(&text);
                }
                state.bump_last_version();
            } else {
                state.push_line(ChatLine::Assistant { text, usage: None });
                state.streaming_assistant = Some(state.messages.len() - 1);
            }
            // Only auto-scroll if the user hasn't manually scrolled away.
            if state.auto_scroll {
                state.scroll_to_bottom();
            }
        }
        AgentEvent::ThinkingDelta(text) => {
            let last_is_thinking = state
                .messages
                .last()
                .is_some_and(|l| matches!(l, ChatLine::Thinking(_)));
            if last_is_thinking {
                if let Some(ChatLine::Thinking(s)) = state.messages.last_mut() {
                    s.push_str(&text);
                }
                state.bump_last_version();
            } else {
                state.push_line(ChatLine::Thinking(text));
            }
            if state.auto_scroll {
                state.scroll_to_bottom();
            }
        }
        AgentEvent::UsageUpdate {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
        } => {
            // Attribute this turn's usage to the assistant line being streamed.
            // Overwriting handles multiple deltas within one stream (output_tokens
            // accumulates); tool-only turns have no assistant line and are skipped.
            if let Some(idx) = state.streaming_assistant {
                if let Some(ChatLine::Assistant { usage, .. }) = state.messages.get_mut(idx) {
                    *usage = Some(TurnUsage {
                        input_tokens,
                        output_tokens,
                        cache_creation_input_tokens,
                        cache_read_input_tokens,
                    });
                }
                state.bump_version_at(idx);
            }
            // `input_tokens` is `Some` only on the final delta of a turn — at that
            // point output_tokens and cache_read_input_tokens also hold the
            // complete per-turn totals. Use this as the signal to capture
            // latest-turn usage (status bar) and update cumulative totals
            // (kept for future session-level display).
            if let Some(t) = input_tokens {
                state.latest_turn_input_tokens = t;
                state.latest_turn_output_tokens = output_tokens;
                state.latest_turn_cache_read_tokens = cache_read_input_tokens.unwrap_or(0);
                state.latest_turn_cache_creation_tokens =
                    cache_creation_input_tokens.unwrap_or(0);
                // Anthropic's `input_tokens` is the count of NEW uncached
                // tokens. To get the full prompt size that counted against
                // the context window, add cached reads + cached writes.
                state.latest_turn_context_used = t
                    + state.latest_turn_cache_read_tokens
                    + state.latest_turn_cache_creation_tokens;
                state.input_tokens += t;
                state.output_tokens += output_tokens;
                if let Some(c) = cache_read_input_tokens {
                    state.cache_read_tokens += c;
                }
            }
        }
        AgentEvent::LlmResponse(_text) => {
            // The full aggregated response — we already have it via TextDelta.
            // No-op here; the assistant line was built incrementally.
        }
        AgentEvent::Thinking(_text) => {
            // Aggregated thinking block — already streamed via ThinkingDelta.
        }
        AgentEvent::ToolCall { name, input } => {
            state.push_line(ChatLine::ToolCall {
                name,
                input: input.to_string(),
            });
            if state.auto_scroll {
                state.scroll_to_bottom();
            }
        }
        AgentEvent::ToolResult { ok, content } => {
            state.push_line(ChatLine::ToolResult { ok, content });
            if state.auto_scroll {
                state.scroll_to_bottom();
            }
        }
        AgentEvent::ToolCallBackground { seq, name } => {
            state.push_line(ChatLine::ToolBackground {
                seq,
                name: name.clone(),
            });
            state.tool_tasks.push(ToolTaskInfo {
                seq,
                name,
                status: ToolTaskStatus::Running,
            });
            if state.auto_scroll {
                state.scroll_to_bottom();
            }
        }
        AgentEvent::ToolBackgroundComplete { seq, ok } => {
            state.push_line(ChatLine::ToolResult {
                ok,
                content: format!("Background task #{seq} has completed"),
            });
            if let Some(task) = state.tool_tasks.iter_mut().find(|t| t.seq == seq) {
                task.status = ToolTaskStatus::Done { ok };
            }
            if state.auto_scroll {
                state.scroll_to_bottom();
            }
        }
        AgentEvent::Done => {
            state
                .tool_tasks
                .retain(|t| matches!(t.status, ToolTaskStatus::Running));
            state.push_line(ChatLine::Separator);
            if state.auto_scroll {
                state.scroll_to_bottom();
            }
        }
        AgentEvent::RetryableError {
            message,
            attempt,
            max_retries,
        } => {
            state.push_line(ChatLine::RetryableError {
                message,
                attempt,
                max_retries,
            });
            if state.auto_scroll {
                state.scroll_to_bottom();
            }
        }
        AgentEvent::Error(msg) => {
            state.push_line(ChatLine::Error(msg));
            if state.auto_scroll {
                state.scroll_to_bottom();
            }
        }
        // Streaming protocol events — not surfaced directly to the chat view
        AgentEvent::StreamStart { .. }
        | AgentEvent::ContentBlockStart { .. }
        | AgentEvent::ContentBlockStop { .. }
        | AgentEvent::StreamDelta { .. } => {}
        AgentEvent::Compact { event } => match event {
            CompactEvent::CompactStart { .. } => state.compact_state.is_compacting = true,
            CompactEvent::CompactFinish { .. } => state.compact_state.is_compacting = false,
        },
        // Session lifecycle events — handled at the AppState level (these
        // need access to the full AgentSession, not just the chat view).
        AgentEvent::SessionActivated { .. }
        | AgentEvent::SessionPaused { .. }
        | AgentEvent::SessionClosed { .. }
        | AgentEvent::SessionList { .. } => {}
        AgentEvent::PlanUpdate { revision, update } => {
            state.plan = PlanState {
                steps: update.plan,
                explanation: update.explanation,
                revision,
            };
            if state.auto_scroll {
                state.scroll_to_bottom();
            }
        }
    }
}

/// Apply session lifecycle events to the full AppState (mutates the
/// `sub_sessions` list of the agent identified by `agent_idx`).
///
/// `agent_idx` is the session-tab index that the event was routed to by
/// agent name.  When it differs from `active_agent_idx` (e.g. a delegation
/// triggers `SessionActivated` on a background agent), only the target
/// agent's sub-sessions are touched — the foreground agent's view is
/// untouched.
pub fn apply_session_event(
    state: &mut AppState,
    event: AgentEvent,
    agent_idx: usize,
) {
    let active_agent_id = state
        .sessions
        .get(agent_idx)
        .map(|s| s.agent_id);
    let active_session_id = state
        .sessions
        .get(agent_idx)
        .and_then(|s| s.sub_sessions.get(s.active_sub_session_idx))
        .map(|s| s.id);
    let session = match state.sessions.get_mut(agent_idx) {
        Some(s) => s,
        None => return,
    };
    match event {
        AgentEvent::SessionList { sessions } => {
            // Build new SubSession entries, preserving tab_state for sessions
            // we already know about (so we don't wipe message history).
            let old_subs = std::mem::take(&mut session.sub_sessions);
            let mut previous: std::collections::HashMap<uuid::Uuid, SubSession> =
                old_subs.into_iter().map(|s| (s.id, s)).collect();
            let subs: Vec<SubSession> = sessions
                .into_iter()
                .map(|info| {
                    if let Some(mut old) = previous.remove(&info.id) {
                        // Keep the existing tab_state, update metadata.
                        old.title = info.title;
                        old.last_active = info.last_active;
                        old.created_at = info.created_at;
                        old
                    } else {
                        let mut sub = SubSession::new(info.id, info.title);
                        sub.last_active = info.last_active;
                        sub.created_at = info.created_at;
                        sub
                    }
                })
                .collect();
            // Feed the picker if it's open for the same agent.
            if state.session_picker.visible
                && state.session_picker.agent_id == active_agent_id
            {
                let picker_items: Vec<crate::widgets::session_picker::PickerSession> =
                    subs.iter()
                        .map(|s| {
                            let stats = crate::widgets::session_picker::compute_session_stats(
                                &s.tab_state.messages,
                            );
                            crate::widgets::session_picker::PickerSession {
                                id: s.id,
                                title: s.title.clone(),
                                message_count: s.tab_state.messages.len(),
                                last_active: s.last_active,
                                created_at: 0, // not tracked on SubSession
                                user_message_count: stats.user_message_count,
                                assistant_message_count: stats.assistant_message_count,
                                tool_call_count: stats.tool_call_count,
                                input_tokens: stats.input_tokens,
                                output_tokens: stats.output_tokens,
                                first_user_message: stats.first_user_message,
                                last_assistant_message: stats.last_assistant_message,
                            }
                        })
                        .collect();
                state.session_picker.set_sessions(picker_items, active_session_id);
            }
            session.sub_sessions = subs;
            if session.active_sub_session_idx >= session.sub_sessions.len()
                && !session.sub_sessions.is_empty()
            {
                session.active_sub_session_idx = session.sub_sessions.len() - 1;
            }
        }
        AgentEvent::SessionActivated { id, title } => {
            if let Some(idx) = session.sub_sessions.iter().position(|s| s.id == id) {
                session.active_sub_session_idx = idx;
                if let Some(t) = title.clone() {
                    session.sub_sessions[idx].title = Some(t);
                }
            } else {
                let mut sub = SubSession::new(id, title);
                // Migrate any pending messages (sent before the backend
                // announced a session) into the new sub-session.
                if !session.pending_tab_state.messages.is_empty() {
                    sub.tab_state = std::mem::take(&mut session.pending_tab_state);
                    tracing::debug!(
                        migrated = sub.tab_state.messages.len(),
                        "migrated pending messages into new sub-session"
                    );
                }
                session.sub_sessions.push(sub);
                session.active_sub_session_idx = session.sub_sessions.len() - 1;
            }
        }
        AgentEvent::SessionPaused { id } => {
            if let Some(s) = session.sub_sessions.iter().find(|s| s.id == id) {
                tracing::debug!(session_id = %s.id, "session paused");
            }
        }
        AgentEvent::SessionClosed { id } => {
            if let Some(pos) = session.sub_sessions.iter().position(|s| s.id == id) {
                session.sub_sessions.remove(pos);
                if session.sub_sessions.is_empty() {
                    session.active_sub_session_idx = 0;
                } else if session.active_sub_session_idx >= session.sub_sessions.len() {
                    session.active_sub_session_idx = session.sub_sessions.len() - 1;
                }
            }
        }
        _ => {}
    }
}

// ── AppState ───────────────────────────────────────────

/// One running agent's UI state.
pub struct AgentSession {
    pub name: String,
    pub agent_id: uuid::Uuid,
    /// All sub-sessions within this agent. Each session carries its own
    /// `tab_state` so switching sessions restores the previous conversation
    /// (messages, token counts, scroll position, …).
    pub sub_sessions: Vec<SubSession>,
    /// Index into `sub_sessions` of the currently active sub-session.
    pub active_sub_session_idx: usize,
    /// Per-agent buffer holding messages sent before the backend announces
    /// the first sub-session (via `SessionActivated`). When a sub-session is
    /// created, the pending state is migrated into it and this is cleared.
    ///
    /// This replaces the old shared `agent_tab_state` fallback, preventing
    /// messages from one agent leaking into another's UI.
    pub pending_tab_state: AgentTabState,
}

/// One conversation within an agent.
pub struct SubSession {
    pub id: uuid::Uuid,
    pub title: Option<String>,
    /// Epoch-millis of the last activity in this session, as reported by
    /// the agent's session list. Drives the picker's "Last active" field.
    pub last_active: i64,
    /// Epoch-millis when this session was created. Used for ordering in the
    /// sidebar session list (newest first).
    pub created_at: i64,
    /// Conversation-level state — chat history, token counts, scroll, etc.
    /// Owned per-session so each session has its own memory of what
    /// happened in it.
    pub tab_state: AgentTabState,
}

impl SubSession {
    pub fn new(id: uuid::Uuid, title: Option<String>) -> Self {
        Self {
            id,
            title,
            last_active: 0,
            created_at: chrono::Utc::now().timestamp_millis(),
            tab_state: AgentTabState::default(),
        }
    }
}

/// Global display preferences affecting how chat lines are rendered.
/// Persisted in the `settings` table.
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct DisplaySettings {
    pub collapse_thinking: bool,
    pub collapse_tool_calls: bool,
    pub collapse_tool_results: bool,
    /// Monotonic counter bumped on every toggle. Used by the render cache
    /// to invalidate all cached lines when display settings change.
    pub version: u64,
}


impl DisplaySettings {
    pub fn toggle_thinking(&mut self) {
        self.collapse_thinking = !self.collapse_thinking;
        self.version = self.version.wrapping_add(1);
    }
    pub fn toggle_tool_calls(&mut self) {
        self.collapse_tool_calls = !self.collapse_tool_calls;
        self.version = self.version.wrapping_add(1);
    }
    pub fn toggle_tool_results(&mut self) {
        self.collapse_tool_results = !self.collapse_tool_results;
        self.version = self.version.wrapping_add(1);
    }
}

/// State container for the TUI.
pub struct AppState {
    /// All active agent sessions. `sessions[active_agent_idx]` is the one
    /// displayed in the workspace and receives user input.
    pub sessions: Vec<AgentSession>,
    pub active_agent_idx: usize,
    /// Available profiles (loaded from storage at startup).
    pub profiles: Vec<agentik_core::AgentProfile>,
    /// Selected index in the profile list within the sidebar.
    pub profile_selected: usize,
    /// Legacy single-agent tab state — kept for backward-compat with
    /// existing code that hasn't been migrated yet. When `sessions` is
    /// non-empty, the active session's `tab_state` is used instead.
    pub agent_tab_state: AgentTabState,
    pub model_config_state: crate::widgets::model_config_widget::ModelConfigState,
    pub command_palette: crate::widgets::command_palette::CommandPaletteState,
    pub profile_picker: crate::widgets::profile_picker::ProfilePickerState,
    pub agent_picker: crate::widgets::agent_picker::AgentPickerState,
    pub name_input: crate::widgets::name_input::NameInputState,
    pub session_picker: crate::widgets::session_picker::SessionPickerState,
    /// When `true`, the name input popup is collecting a **session name**
    /// (not an agent name). The Enter handler checks this flag.
    pub pending_session_name: bool,
    /// When `Some(id)`, the name input popup is collecting a **new title**
    /// for renaming an existing session.
    pub pending_session_rename_id: Option<uuid::Uuid>,
    /// Profile selected from the picker, waiting for the user to enter a name.
    /// When `Some`, the name input popup is shown.
    pub pending_profile: Option<agentik_core::AgentProfile>,
    /// Model config popup visibility.
    pub model_config_visible: bool,
    /// Global display preferences (collapse thinking/tool blocks).
    pub display_settings: DisplaySettings,
    pub active_model: Arc<ArcSwapOption<Model>>,
    /// When `true`, the delete-active-agent confirmation popup is shown.
    /// The user must press 'y' or Enter to confirm, 'n' or Esc to cancel.
    pub delete_agent_confirm: bool,
    /// When `Some(name)`, the next [`HostEvent::AgentRegistered`] whose
    /// short name matches will steal focus to the newly-created leaf.
    /// Set when the user explicitly invokes `spawn_agent_from_profile`
    /// (via the agent picker or the profile picker + name input). Cleared
    /// after the matching registration arrives, on spawn failure, or on
    /// pick-list close so stale flags never mis-route focus.
    pub pending_focus_agent_name: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            active_agent_idx: 0,
            profiles: Vec::new(),
            profile_selected: 0,
            agent_tab_state: AgentTabState::default(),
            model_config_state: Default::default(),
            command_palette: Default::default(),
            profile_picker: Default::default(),
            agent_picker: Default::default(),
            name_input: Default::default(),
            session_picker: Default::default(),
            pending_session_name: false,
            pending_session_rename_id: None,
            pending_profile: None,
            model_config_visible: false,
            display_settings: DisplaySettings::default(),
            active_model: Arc::new(ArcSwapOption::default()),
            delete_agent_confirm: false,
            pending_focus_agent_name: None,
        }
    }
}

impl AppState {
    /// Returns a mutable reference to the active sub-session's tab state.
    /// If the active agent exists but has no sub-sessions yet (before the
    /// first `SessionActivated`), returns that agent's `pending_tab_state`
    /// so messages are buffered per-agent without cross-contamination.
    /// If no agent exists at all, returns the legacy `agent_tab_state`
    /// (which is never displayed since the workspace renders an empty state).
    pub fn active_tab_state_mut(&mut self) -> &mut AgentTabState {
        if let Some(session) = self.sessions.get_mut(self.active_agent_idx) {
            if let Some(sub) = session
                .sub_sessions
                .get_mut(session.active_sub_session_idx)
            {
                return &mut sub.tab_state;
            }
            return &mut session.pending_tab_state;
        }
        &mut self.agent_tab_state
    }

    /// Returns an immutable reference to the active sub-session's tab state.
    pub fn active_tab_state(&self) -> &AgentTabState {
        if let Some(session) = self.sessions.get(self.active_agent_idx) {
            if let Some(sub) = session
                .sub_sessions
                .get(session.active_sub_session_idx)
            {
                return &sub.tab_state;
            }
            return &session.pending_tab_state;
        }
        &self.agent_tab_state
    }

    /// Returns the status of the active session (or Idle if none).
    pub fn active_status(&self) -> AgentStatus {
        self.active_tab_state().status.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_queue_enqueue_and_drain() {
        let mut ts = AgentTabState::default();
        assert_eq!(ts.pending_queue_len(), 0);
        assert!(ts.drain_pending_queue().is_empty());

        ts.enqueue_pending("hello".into());
        ts.enqueue_pending("world".into());
        assert_eq!(ts.pending_queue_len(), 2);

        let drained = ts.drain_pending_queue();
        assert_eq!(drained, vec!["hello", "world"]);
        assert_eq!(ts.pending_queue_len(), 0);
    }

    #[test]
    fn can_send_vs_can_enqueue() {
        let mut ts = AgentTabState::default();
        // Idle + empty → neither
        assert!(!ts.can_send());
        assert!(!ts.can_enqueue());

        // Idle + text → can send (immediate)
        ts.input.insert_str("hi");
        assert!(ts.can_send());
        assert!(ts.can_enqueue());

        // Running + text → can enqueue only
        ts.status = AgentStatus::Streaming;
        assert!(!ts.can_send());
        assert!(ts.can_enqueue());

        // Running + empty → neither
        ts.input.clear();
        assert!(!ts.can_send());
        assert!(!ts.can_enqueue());
    }

    #[test]
    fn apply_done_does_not_reset_status_under_unified_contract() {
        // Under the unified lifecycle contract, the core layer emits
        // `LifecycleChanged(Idle)` as the authoritative status event before
        // `Done`. `apply_event(Done)` itself must NOT touch `status` — that
        // would clobber an error state emitted via `LifecycleChanged(Error)`.
        let mut ts = AgentTabState::default();
        ts.status = AgentStatus::Streaming;
        apply_event(&mut ts, AgentEvent::Done);
        assert_eq!(ts.status, AgentStatus::Streaming);

        // `LifecycleChanged` is the only path that mutates `status`.
        apply_event(&mut ts, AgentEvent::LifecycleChanged(AgentStatus::Idle));
        assert_eq!(ts.status, AgentStatus::Idle);
        assert_eq!(ts.pending_queue_len(), 0);
    }
}
