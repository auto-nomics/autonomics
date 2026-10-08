use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    ContentBlockDelta, Message, MessageStreamEvent, SessionTelemetry, StopReason, TurnTelemetry,
};

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

    /// A conversation turn started. Unlike `Done`, this carries stable
    /// identities that can be correlated with a caller's delegation request.
    TurnStarted {
        turn_id: Uuid,
        session_id: Uuid,
        delegation_id: Option<Uuid>,
    },

    /// A conversation turn reached a terminal state. This is emitted before
    /// the compatibility `Done`/`Error` event so response buffers remain
    /// available to the host.
    TurnCompleted {
        turn_id: Uuid,
        session_id: Uuid,
        delegation_id: Option<Uuid>,
        status: TurnExecutionStatus,
        #[serde(default)]
        telemetry: TurnTelemetry,
    },

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

    /// The user intentionally interrupted the current turn (Ctrl+C).
    /// Also emits `LifecycleChanged(Cancelled)`.
    ///
    /// Semantically distinct from `Error`: the agent didn't fail, the user
    /// chose to stop. A conversation marker is injected into memory so the
    /// LLM knows the previous turn was interrupted.
    TurnAborted,

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

    /// A user message was injected into the conversation by an external source
    /// (delegate_to or background-task completion notice).
    MessageInjected(String),

    /// A user-submitted message was committed to conversation memory. The UI
    /// uses this acknowledgement to promote a pending prompt into the visible
    /// transcript after a safe response/tool boundary.
    UserMessageAcknowledged(String),

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
    #[serde(default)]
    pub telemetry: SessionTelemetry,
}

/// Progress reporting for one context-compaction pass.
///
/// A pass always emits a `CompactStart` / `CompactFinish` pair — including
/// on failure and when the conversation turns out to be too short to
/// compact — so frontends can drive their progress UI from a closed event
/// pair and never hang in a "compacting" state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompactEvent {
    /// Compaction began. `plan` describes what is about to happen; it is
    /// `None` when the conversation is too short to compact (the pass ends
    /// immediately with a stats-less `CompactFinish`).
    CompactStart {
        ts: DateTime<Utc>,
        #[serde(default)]
        plan: Option<CompactPlan>,
    },

    /// A phase transition inside the pass. `Summarizing` precedes the LLM
    /// summarization call (the slow part); `Rebuilding` precedes the
    /// in-place message-list rewrite. The local head/tail selection is
    /// millisecond-scale and folded into `CompactStart`'s `plan`.
    CompactPhase {
        ts: DateTime<Utc>,
        phase: CompactPhase,
    },

    /// A throttled chunk of the compaction summary being generated, for
    /// live preview in frontends. Coalesced at the source (~120 chars or
    /// ~200 ms) so the event channel and SSE replay ring are not flooded.
    CompactSummaryDelta { ts: DateTime<Utc>, text: String },

    /// Compaction ended — successfully (`stats`), without work to do
    /// (neither field), or with an error (`error`).
    CompactFinish {
        ts: DateTime<Utc>,
        #[serde(default)]
        stats: Option<CompactStats>,
        #[serde(default)]
        error: Option<String>,
    },
}

/// What initiated a compaction pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactTrigger {
    /// User-issued compact command.
    Manual,
    /// Context pressure detected after tool execution, before the
    /// follow-up request. `used_pct` is the estimated context fill.
    MidTurn { used_pct: u64 },
    /// Context pressure detected before a request (at or above the
    /// auto-compact threshold of the model's context window).
    PreRequest { used_pct: u64 },
}

/// Coarse phase of an in-flight compaction pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactPhase {
    /// The LLM is generating the head summary (the slow part).
    Summarizing,
    /// The head/tail split is applied and the session rebuilt + persisted.
    Rebuilding,
}

/// Snapshot of what a compaction pass will do, emitted with
/// `CompactEvent::CompactStart`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactPlan {
    /// What initiated this pass.
    pub trigger: CompactTrigger,
    /// Number of head messages that will be summarized away.
    pub head_messages: usize,
    /// Estimated tokens of the summarized head.
    pub head_tokens: u64,
    /// Number of recent tail messages kept verbatim.
    pub tail_messages: usize,
}

/// Outcome statistics of a completed compaction pass, emitted with
/// `CompactEvent::CompactFinish`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompactStats {
    /// Conversation length (messages) before the pass.
    pub messages_before: usize,
    /// Conversation length (messages) after the pass.
    pub messages_after: usize,
    /// Estimated tokens of the generated summary.
    pub summary_tokens: u64,
    /// Estimated tokens removed from the model context
    /// (`head_tokens` minus preserved user messages minus the summary).
    pub freed_tokens: u64,
    /// Wall-clock duration of the whole pass.
    pub duration_ms: u64,
    /// Actual API usage of the summarization call.
    #[serde(default)]
    pub usage: crate::shared::Usage,
}

/// Terminal status of an explicitly tracked conversation turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnExecutionStatus {
    Completed,
    Interrupted,
    Failed,
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

#[cfg(test)]
mod compact_event_tests {
    use super::*;

    /// Events serialized by an older build (no `plan` / `stats` / `error`
    /// fields) must still deserialize — persisted logs and in-flight SSE
    /// payloads outlive the binary that emitted them.
    #[test]
    fn compact_events_deserialize_from_legacy_payloads() {
        let ts = "2026-09-28T12:00:00Z";
        let start: CompactEvent =
            serde_json::from_str(&format!("{{\"CompactStart\":{{\"ts\":\"{ts}\"}}}}")).unwrap();
        assert!(matches!(
            start,
            CompactEvent::CompactStart { plan: None, .. }
        ));

        let finish: CompactEvent =
            serde_json::from_str(&format!("{{\"CompactFinish\":{{\"ts\":\"{ts}\"}}}}")).unwrap();
        assert!(matches!(
            finish,
            CompactEvent::CompactFinish {
                stats: None,
                error: None,
                ..
            }
        ));
    }

    /// The full progress sequence survives a JSON round trip.
    #[test]
    fn compact_progress_events_round_trip() {
        let ts = Utc::now();
        let events = vec![
            CompactEvent::CompactStart {
                ts,
                plan: Some(CompactPlan {
                    trigger: CompactTrigger::PreRequest { used_pct: 93 },
                    head_messages: 12,
                    head_tokens: 38_412,
                    tail_messages: 8,
                }),
            },
            CompactEvent::CompactPhase {
                ts,
                phase: CompactPhase::Summarizing,
            },
            CompactEvent::CompactSummaryDelta {
                ts,
                text: "## Progress".into(),
            },
            CompactEvent::CompactPhase {
                ts,
                phase: CompactPhase::Rebuilding,
            },
            CompactEvent::CompactFinish {
                ts,
                stats: Some(CompactStats {
                    messages_before: 45,
                    messages_after: 12,
                    summary_tokens: 1_204,
                    freed_tokens: 36_800,
                    duration_ms: 12_345,
                    usage: Default::default(),
                }),
                error: None,
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let back: CompactEvent = serde_json::from_str(&json).unwrap();
            let same = match (&event, &back) {
                (
                    CompactEvent::CompactStart { plan: a, .. },
                    CompactEvent::CompactStart { plan: b, .. },
                ) => a == b,
                (
                    CompactEvent::CompactPhase { phase: a, .. },
                    CompactEvent::CompactPhase { phase: b, .. },
                ) => a == b,
                (
                    CompactEvent::CompactSummaryDelta { text: a, .. },
                    CompactEvent::CompactSummaryDelta { text: b, .. },
                ) => a == b,
                (
                    CompactEvent::CompactFinish {
                        stats: a, error: x, ..
                    },
                    CompactEvent::CompactFinish {
                        stats: b, error: y, ..
                    },
                ) => a == b && x == y,
                _ => false,
            };
            assert!(same, "round-trip mismatch: {event:?} vs {back:?}");
        }
    }
}
