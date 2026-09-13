//! External run-event contract for headless execution.
//!
//! This schema is the stable wire format emitted on stdout in `--json`
//! mode (one JSON object per line). It is deliberately defined here,
//! independent of `agentik_types::AgentEvent`, so the internal event enum
//! can evolve without breaking external consumers — the same separation
//! codex's `exec_events.rs` keeps from its protocol types.
//!
//! Naming follows codex: dot-namespaced event tags (`turn.started`,
//! `item.completed`, …) with items carrying typed payloads. Token-level
//! deltas are intentionally absent: items are aggregated (an agent
//! message arrives once, complete), matching codex's exec item model —
//! streaming is a presentation concern that lives in the human
//! processor, not in the contract.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Top-level event emitted by a headless run; one JSON object per line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum RunEvent {
    /// First event of every run.
    #[serde(rename = "run.started")]
    RunStarted(RunStartedEvent),

    /// A conversation turn began processing the prompt.
    #[serde(rename = "turn.started")]
    TurnStarted(TurnStartedEvent),

    /// An item (tool call, …) began executing.
    #[serde(rename = "item.started")]
    ItemStarted(ItemEvent),

    /// An item reached a terminal state, or an aggregated item (agent
    /// message, reasoning) was produced.
    #[serde(rename = "item.completed")]
    ItemCompleted(ItemEvent),

    /// The turn finished successfully.
    #[serde(rename = "turn.completed")]
    TurnCompleted(TurnCompletedEvent),

    /// The turn reached a terminal error state.
    #[serde(rename = "turn.failed")]
    TurnFailed(TurnFailedEvent),

    /// A non-fatal runtime happening (compaction, plan update, retry, …).
    #[serde(rename = "notice")]
    Notice(NoticeEvent),

    /// Last event of every run.
    #[serde(rename = "run.ended")]
    RunEnded(RunEndedEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunStartedEvent {
    /// Agent id assigned by the runtime (stable across restarts via storage).
    pub agent_id: Uuid,
    /// Session the run executes in.
    pub session_id: Uuid,
    /// Profile path the agent was spawned from.
    pub profile: String,
    /// Model name, when known at start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnStartedEvent {
    pub turn_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnCompletedEvent {
    pub turn_id: Uuid,
    /// Cumulative token usage for the turn.
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnFailedEvent {
    pub turn_id: Uuid,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunEndedEvent {
    pub status: RunStatus,
    pub wall_time_secs: f64,
    /// Cumulative usage across all turns, when any was reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Number of completed/failed turns in this run.
    pub turns: u64,
}

/// Terminal status of the whole run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Failed,
    Cancelled,
}

/// Token usage for a turn or a whole run.
///
/// `input_tokens` and the cache fields are `Option` because the streaming
/// protocol reports them only on the final delta of a turn.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

/// An item happening inside a turn, with its typed payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ItemEvent {
    pub item: RunItem,
}

/// Canonical representation of a run item and its domain payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunItem {
    /// Stable id for correlating `item.started` with `item.completed`.
    pub id: String,
    #[serde(flatten)]
    pub details: RunItemDetails,
}

/// Typed payloads for each supported item kind.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunItemDetails {
    /// Complete response from the agent.
    AgentMessage(AgentMessageItem),
    /// The agent's reasoning (thinking) block.
    Reasoning(ReasoningItem),
    /// A tool invocation. `started` carries the request; `completed`
    /// carries the request plus result.
    ToolCall(ToolCallItem),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentMessageItem {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReasoningItem {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallItem {
    /// Registered tool name (e.g. `run_bash`).
    pub tool: String,
    /// Raw JSON arguments as sent by the model.
    pub input: Value,
    /// Raw text result — present once the call finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Whether the tool reported success — present once finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
}

/// Kind of a non-fatal notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    /// Context compaction started/finished.
    Compact,
    /// The persistent task plan was updated.
    PlanUpdate,
    /// A retryable error occurred; the agent backs off and retries.
    RetryableError,
    /// A tool moved to background execution.
    BackgroundTool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoticeEvent {
    pub kind: NoticeKind,
    /// Human-readable one-liner.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_tags_are_dot_namespaced() {
        let turn_id = Uuid::nil();
        let event = RunEvent::TurnStarted(TurnStartedEvent { turn_id });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "turn.started");
        assert_eq!(json["turn_id"], turn_id.to_string());
    }

    #[test]
    fn item_payload_is_flattened_with_snake_case_tag() {
        let item = ItemEvent {
            item: RunItem {
                id: "t1".into(),
                details: RunItemDetails::ToolCall(ToolCallItem {
                    tool: "run_bash".into(),
                    input: serde_json::json!({"cmd": "ls"}),
                    result: None,
                    ok: None,
                }),
            },
        };
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["item"]["id"], "t1");
        assert_eq!(json["item"]["type"], "tool_call");
        assert_eq!(json["item"]["tool"], "run_bash");
        // Optional fields are absent while the call is in flight.
        assert!(json["item"].get("result").is_none());
        assert!(json["item"].get("ok").is_none());
    }

    #[test]
    fn every_variant_round_trips() {
        let events = vec![
            RunEvent::RunStarted(RunStartedEvent {
                agent_id: Uuid::nil(),
                session_id: Uuid::nil(),
                profile: "default".into(),
                model: Some("test-model".into()),
            }),
            RunEvent::TurnStarted(TurnStartedEvent {
                turn_id: Uuid::nil(),
            }),
            RunEvent::ItemStarted(ItemEvent {
                item: RunItem {
                    id: "t1".into(),
                    details: RunItemDetails::ToolCall(ToolCallItem {
                        tool: "run_bash".into(),
                        input: Value::Null,
                        result: None,
                        ok: None,
                    }),
                },
            }),
            RunEvent::ItemCompleted(ItemEvent {
                item: RunItem {
                    id: "t1".into(),
                    details: RunItemDetails::ToolCall(ToolCallItem {
                        tool: "run_bash".into(),
                        input: Value::Null,
                        result: Some("done".into()),
                        ok: Some(true),
                    }),
                },
            }),
            RunEvent::ItemCompleted(ItemEvent {
                item: RunItem {
                    id: "m1".into(),
                    details: RunItemDetails::AgentMessage(AgentMessageItem {
                        text: "hello".into(),
                    }),
                },
            }),
            RunEvent::ItemCompleted(ItemEvent {
                item: RunItem {
                    id: "r1".into(),
                    details: RunItemDetails::Reasoning(ReasoningItem {
                        text: "hmm".into(),
                    }),
                },
            }),
            RunEvent::TurnCompleted(TurnCompletedEvent {
                turn_id: Uuid::nil(),
                usage: Usage::default(),
            }),
            RunEvent::TurnFailed(TurnFailedEvent {
                turn_id: Uuid::nil(),
                message: "boom".into(),
            }),
            RunEvent::Notice(NoticeEvent {
                kind: NoticeKind::Compact,
                message: "compacting".into(),
            }),
            RunEvent::RunEnded(RunEndedEvent {
                status: RunStatus::Completed,
                wall_time_secs: 1.5,
                usage: Some(Usage {
                    input_tokens: Some(10),
                    output_tokens: 20,
                    ..Default::default()
                }),
                turns: 1,
            }),
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let back: RunEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, event, "round-trip failed for {json}");
        }
    }
}
