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
    /// Invocation id, shared with the optional on-disk run manifest.
    #[serde(default)]
    pub run_id: Uuid,
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
    /// The session the turn runs in — the handle scripts use to resume
    /// later via `--session`.
    pub session_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnCompletedEvent {
    pub turn_id: Uuid,
    /// Cumulative token usage for the turn.
    pub usage: Usage,
    /// Authoritative resource telemetry for the turn.
    #[serde(default)]
    pub telemetry: Telemetry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnFailedEvent {
    pub turn_id: Uuid,
    pub message: String,
    /// Best-known telemetry for the failed turn.
    #[serde(default)]
    pub telemetry: Telemetry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunEndedEvent {
    /// Invocation id from `run.started`.
    #[serde(default)]
    pub run_id: Uuid,
    pub status: RunStatus,
    pub wall_time_secs: f64,
    /// Cumulative usage across all turns, when any was reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Number of completed/failed turns in this run.
    pub turns: u64,
    /// Number of tool calls that completed during the run.
    pub tool_calls: u64,
    /// Aggregated resource telemetry for this invocation.
    #[serde(default)]
    pub telemetry: Telemetry,
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

/// Resource telemetry for a turn or a complete headless invocation.
///
/// Unlike `Usage`, this includes tool activity and active agent work time.
/// `time_consume_ms` excludes time spent waiting for background tools.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Telemetry {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub total_tokens: u64,
    pub llm_call_count: u64,
    pub total_tool_use: u64,
    pub tool_result_count: u64,
    pub failed_tool_result_count: u64,
    pub time_consume_ms: u64,
}

impl From<agentik_types::TurnTelemetry> for Telemetry {
    fn from(value: agentik_types::TurnTelemetry) -> Self {
        Self {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cache_read_input_tokens: value.cache_read_input_tokens,
            cache_creation_input_tokens: value.cache_creation_input_tokens,
            total_tokens: value.total_tokens,
            llm_call_count: value.llm_call_count,
            total_tool_use: value.total_tool_use,
            tool_result_count: value.tool_result_count,
            failed_tool_result_count: value.failed_tool_result_count,
            time_consume_ms: value.time_consume_ms,
        }
    }
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
        let event = RunEvent::TurnStarted(TurnStartedEvent {
            turn_id,
            session_id: Uuid::nil(),
        });
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
                run_id: Uuid::nil(),
                agent_id: Uuid::nil(),
                session_id: Uuid::nil(),
                profile: "default".into(),
                model: Some("test-model".into()),
            }),
            RunEvent::TurnStarted(TurnStartedEvent {
                turn_id: Uuid::nil(),
                session_id: Uuid::nil(),
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
                    details: RunItemDetails::Reasoning(ReasoningItem { text: "hmm".into() }),
                },
            }),
            RunEvent::TurnCompleted(TurnCompletedEvent {
                turn_id: Uuid::nil(),
                usage: Usage::default(),
                telemetry: Telemetry::default(),
            }),
            RunEvent::TurnFailed(TurnFailedEvent {
                turn_id: Uuid::nil(),
                message: "boom".into(),
                telemetry: Telemetry::default(),
            }),
            RunEvent::Notice(NoticeEvent {
                kind: NoticeKind::Compact,
                message: "compacting".into(),
            }),
            RunEvent::RunEnded(RunEndedEvent {
                run_id: Uuid::nil(),
                status: RunStatus::Completed,
                wall_time_secs: 1.5,
                usage: Some(Usage {
                    input_tokens: Some(10),
                    output_tokens: 20,
                    ..Default::default()
                }),
                turns: 1,
                tool_calls: 2,
                telemetry: Telemetry::default(),
            }),
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let back: RunEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, event, "round-trip failed for {json}");
        }
    }

    #[test]
    fn terminal_events_deserialize_without_telemetry() {
        let completed: RunEvent = serde_json::from_str(
            r#"{
                "type": "turn.completed",
                "turn_id": "00000000-0000-0000-0000-000000000000",
                "usage": {
                    "input_tokens": null,
                    "output_tokens": 0,
                    "cache_read_input_tokens": null,
                    "cache_creation_input_tokens": null
                }
            }"#,
        )
        .unwrap();
        let telemetry = match completed {
            RunEvent::TurnCompleted(TurnCompletedEvent { telemetry, .. }) => telemetry,
            other => panic!("expected turn.completed, got {other:?}"),
        };
        assert_eq!(telemetry, Telemetry::default());

        let ended: RunEvent = serde_json::from_str(
            r#"{
                "type": "run.ended",
                "run_id": "00000000-0000-0000-0000-000000000000",
                "status": "completed",
                "wall_time_secs": 1.0,
                "turns": 1,
                "tool_calls": 0
            }"#,
        )
        .unwrap();
        let telemetry = match ended {
            RunEvent::RunEnded(RunEndedEvent { telemetry, .. }) => telemetry,
            other => panic!("expected run.ended, got {other:?}"),
        };
        assert_eq!(telemetry, Telemetry::default());
    }
}
