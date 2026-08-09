use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{ContentBlockDelta, Message, MessageStreamEvent, StopReason};

/// Unified event emitted by the agent for external observation (TUI, logging, etc.).
///
/// This enum covers the full lifecycle of an agent run:
/// - **Real-time streaming deltas** — token-level updates translated from SSE events
/// - **Aggregated responses** — complete LLM output emitted after a stream finishes
/// - **Agent lifecycle** — tool calls, tool results, completion, errors
///
/// Consumers subscribe to a single `broadcast::Receiver<AgentEvent>` and filter
/// on the variants they care about.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AgentEvent {
    // ── Real-time streaming deltas (translated from MessageStreamEvent) ──
    /// A text token arrived from the LLM.
    TextDelta(String),

    /// A thinking/reasoning token arrived from the LLM.
    ThinkingDelta(String),

    /// Token usage updated mid-stream.
    UsageUpdate {
        input_tokens: Option<u64>,
        output_tokens: u64,
        cache_creation_input_tokens: Option<u64>,
        cache_read_input_tokens: Option<u64>,
    },

    /// The LLM stream started (carries initial message metadata).
    StreamStart { message: Message },

    /// A content block began (text, thinking, `tool_use`, …).
    ContentBlockStart {
        index: usize,
        content_block_kind: ContentBlockKind,
    },

    /// A content block ended.
    ContentBlockStop { index: usize },

    /// The LLM indicated a stop reason (`end_turn`, `tool_use`, `max_tokens`, …).
    StreamDelta { stop_reason: Option<StopReason> },

    // ── Aggregated LLM responses (emitted after stream completes) ──
    /// LLM produced a complete text response (all text blocks concatenated).
    LlmResponse(String),

    /// LLM produced a complete thinking block.
    Thinking(String),

    // ── Agent lifecycle ──
    /// The agent's lifecycle status changed. This is the **authoritative**
    /// source for the agent's current status — consumers should track
    /// status from this event rather than inferring it from other events
    /// (Requesting, TextDelta, Done, etc.) which carry additional
    /// domain-specific data but are not a reliable status signal.
    LifecycleChanged(crate::AgentLifecycleStatus),

    /// Agent is about to call the LLM API (waiting for response).
    /// Also emits `LifecycleChanged(Requesting)`.
    Requesting,

    /// Agent is calling a tool. `input` carries the raw JSON arguments.
    ToolCall { name: String, input: Value },

    /// A tool returned a result. `content` is the raw text from the tool.
    ToolResult { ok: bool, content: String },

    /// Sync phase expired — tool is now running in the background.
    /// `seq` is the short task number (1-based), `name` is the tool name.
    ToolCallBackground { seq: u64, name: String },

    /// A background tool task completed with its real result.
    ToolBackgroundComplete { seq: u64, ok: bool },

    /// Agent is performing context compaction.
    /// Also emits `LifecycleChanged(Compacting)` / `LifecycleChanged(Requesting)`.
    Compact { event: CompactEvent },

    /// The agent's persistent task plan was updated via `update_plan`.
    /// Carries the full new plan snapshot and the revision number.
    PlanUpdate {
        revision: u64,
        update: crate::plan::PlanUpdate,
    },

    /// Agent finished its workflow.
    Done,

    /// A retryable error occurred. The agent will back off and retry.
    ///
    /// `attempt` is 1-based (the attempt that just failed);
    /// `max_retries` is the configured ceiling.
    RetryableError {
        message: String,
        attempt: u32,
        max_retries: u32,
    },

    /// An error occurred.
    Error(String),

    // ── Session lifecycle ──
    /// A session was activated (became the active session).
    SessionActivated { id: Uuid, title: Option<String> },
    /// A session was paused (no longer the active session).
    SessionPaused { id: Uuid },
    /// A session was closed and removed.
    SessionClosed { id: Uuid },
    /// Response to `ListSessions` — all known sessions.
    SessionList { sessions: Vec<SessionInfo> },
}

/// Lightweight info about a session, for listing / display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: Uuid,
    pub title: Option<String>,
    pub created_at: i64,
    pub last_active: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompactEvent {
    CompactStart { ts: DateTime<Utc> },
    CompactFinish { ts: DateTime<Utc> },
}

/// Coarse-grained kind of a content block, sufficient for UI observation
/// without exposing the full `ContentBlock` details.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ContentBlockKind {
    Text,
    Thinking,
    ToolUse {
        name: String,
    },
    /// Catch-all for image or other rare block types.
    Other,
}

impl From<&crate::ContentBlock> for ContentBlockKind {
    fn from(block: &crate::ContentBlock) -> Self {
        match block {
            crate::ContentBlock::Text { .. } => ContentBlockKind::Text,
            crate::ContentBlock::Thinking { .. } => ContentBlockKind::Thinking,
            crate::ContentBlock::ToolUse { name, .. } => {
                ContentBlockKind::ToolUse { name: name.clone() }
            }
            _ => ContentBlockKind::Other,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Conversion: wire-level SSE event → unified AgentEvent
// ─────────────────────────────────────────────────────────────

impl AgentEvent {
    /// Translate a wire-level SSE event into an agent-level event.
    ///
    /// Returns `None` for events that carry no useful information for
    /// external observers (e.g. `MessageStop` — the agent emits `Done`
    /// itself based on lifecycle state, not on the stream protocol).
    #[must_use]
    pub fn from_stream_event(event: &MessageStreamEvent) -> Option<Self> {
        match event {
            MessageStreamEvent::MessageStart { message } => Some(AgentEvent::StreamStart {
                message: message.clone(),
            }),

            MessageStreamEvent::ContentBlockStart {
                content_block,
                index,
            } => Some(AgentEvent::ContentBlockStart {
                index: *index,
                content_block_kind: ContentBlockKind::from(content_block),
            }),

            MessageStreamEvent::ContentBlockDelta { delta, .. } => match delta {
                ContentBlockDelta::TextDelta { text } => Some(AgentEvent::TextDelta(text.clone())),
                ContentBlockDelta::ThinkingDelta { thinking } => {
                    Some(AgentEvent::ThinkingDelta(thinking.clone()))
                }
                // InputJsonDelta, CitationsDelta, SignatureDelta — internal protocol
                // details, not surfaced to agent-level observers.
                _ => None,
            },

            MessageStreamEvent::MessageDelta { usage, .. } => Some(AgentEvent::UsageUpdate {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_creation_input_tokens: usage.cache_creation_input_tokens,
                cache_read_input_tokens: usage.cache_read_input_tokens,
            }),

            MessageStreamEvent::ContentBlockStop { index } => {
                Some(AgentEvent::ContentBlockStop { index: *index })
            }

            MessageStreamEvent::MessageStop => {
                // The agent emits `Done` based on lifecycle, not on the SSE protocol.
                None
            }
        }
    }
}
