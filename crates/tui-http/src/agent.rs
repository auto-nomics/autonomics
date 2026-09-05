//! SSE bridge utilities for the agentik chat engine.
//!
//! The per-request ephemeral agent this module once built (`POST /chat`,
//! retired in P4 — see `docs/design/web-agent-runtime.md` §12) was replaced
//! by the resident-agent module (`agent_runtime.rs`, feature `runtime-host`):
//! one long-lived agent per `agent_type`, threads mapped onto agentik
//! sessions. What remains here is the wire mapping both protocols share —
//! `agentik` events to the frontend's SSE frames — kept as pure functions so
//! it stays unit-testable.

use std::collections::VecDeque;
use std::time::Duration;

use agentik_sdk::types::AgentEvent;
use axum::response::sse::Event;
use serde_json::json;
use uuid::Uuid;

/// How often the stream emits an explicit `ping` event. The frontend resets
/// its 30 s idle timer on every *parsed event* — a `:`-comment keep-alive is
/// invisible to its parser, so the ping must be a named event.
pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(10);

/// Tool-result previews are capped so a chatty `lit_search` cannot flood the
/// SSE stream; the frontend renders previews, not full payloads.
const TOOL_RESULT_PREVIEW_CHARS: usize = 500;

pub(crate) enum Mapped {
    /// Forward an SSE event, or stay silent for events the frontend does not
    /// know about.
    Continue(Option<Frame>),
    /// Terminal: emit this event and close the stream.
    Finished(Frame),
}

/// A wire frame in the frontend's SSE protocol, kept as structured data so
/// the mapping is unit-testable (axum `Event` cannot be rendered back to
/// text).
pub(crate) struct Frame {
    pub(crate) event: &'static str,
    pub(crate) data: serde_json::Value,
}

/// Build an SSE frame with a JSON data payload (`Event::data` takes a string,
/// not a `Value`).
pub(crate) fn sse(event: &'static str, data: impl serde::Serialize) -> Event {
    Event::default()
        .event(event)
        .data(serde_json::to_string(&data).expect("SSE payloads serialize"))
}

pub(crate) fn frame_to_event(frame: Frame) -> Event {
    sse(frame.event, frame.data)
}

/// Translate an agentik [`AgentEvent`] into the frontend's SSE protocol:
/// `text_delta` / `tool_call_start` / `tool_call_result` / `done` / `error`.
pub(crate) fn map_agent_event(event: AgentEvent, pending_tools: &mut VecDeque<String>) -> Mapped {
    match event {
        AgentEvent::TextDelta(text) => Mapped::Continue(Some(Frame {
            event: "text_delta",
            data: json!({ "text": text }),
        })),

        // The frontend correlates start/result pairs by `tool_use_id`, which
        // agentik's events do not carry. Tools execute sequentially within a
        // turn, so FIFO pairing is exact for the synchronous path.
        AgentEvent::ToolCall { name, input } => {
            let id = Uuid::new_v4().to_string();
            pending_tools.push_back(id.clone());
            Mapped::Continue(Some(Frame {
                event: "tool_call_start",
                data: json!({
                    "tool_use_id": id,
                    "name": name,
                    "input": input,
                }),
            }))
        }
        AgentEvent::ToolResult { ok, content } => {
            let id = pending_tools
                .pop_front()
                .unwrap_or_else(|| "unknown".to_owned());
            Mapped::Continue(Some(Frame {
                event: "tool_call_result",
                data: json!({
                    "tool_use_id": id,
                    "is_error": !ok,
                    "preview": truncate(&content, TOOL_RESULT_PREVIEW_CHARS),
                }),
            }))
        }

        // Background tools have a sequence number instead of a use id; give
        // them a stable synthetic id so the UI card still pairs up.
        AgentEvent::ToolCallBackground { seq, name } => Mapped::Continue(Some(Frame {
            event: "tool_call_start",
            data: json!({
                "tool_use_id": format!("bg-{seq}"),
                "name": name,
                "input": json!({ "background": true }),
            }),
        })),
        AgentEvent::ToolBackgroundComplete { seq, ok } => Mapped::Continue(Some(Frame {
            event: "tool_call_result",
            data: json!({
                "tool_use_id": format!("bg-{seq}"),
                "is_error": !ok,
                "preview": "background task completed",
            }),
        })),

        AgentEvent::Error(message) => Mapped::Finished(Frame {
            event: "error",
            data: json!({
                "message": message,
                "error_code": "",
            }),
        }),

        // TurnCompleted/TurnAborted are followed by the compatibility
        // Done/Error events, so terminal handling stays on those.
        AgentEvent::Done | AgentEvent::TurnAborted => Mapped::Finished(Frame {
            event: "done",
            data: json!({}),
        }),

        // Thinking deltas, usage counters, session bookkeeping: the frontend
        // has no UI for these.
        _ => Mapped::Continue(None),
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_delta_carries_the_token() {
        let mut pending = VecDeque::new();
        let Mapped::Continue(Some(frame)) = map_agent_event(
            AgentEvent::TextDelta("你好".to_owned()),
            &mut pending,
        ) else {
            panic!("TextDelta must map to a forwarded event");
        };
        assert_eq!(frame.event, "text_delta");
        assert_eq!(frame.data["text"], "你好");
    }

    #[test]
    fn tool_calls_pair_up_in_fifo_order() {
        let mut pending = VecDeque::new();
        let Mapped::Continue(Some(start)) = map_agent_event(
            AgentEvent::ToolCall {
                name: "lit_search".to_owned(),
                input: json!({ "query": "gwas" }),
            },
            &mut pending,
        ) else {
            panic!("ToolCall must map to tool_call_start");
        };
        assert_eq!(start.event, "tool_call_start");
        assert_eq!(start.data["name"], "lit_search");
        let id = start.data["tool_use_id"].as_str().unwrap().to_owned();

        let Mapped::Continue(Some(result)) = map_agent_event(
            AgentEvent::ToolResult {
                ok: false,
                content: "boom".to_owned(),
            },
            &mut pending,
        ) else {
            panic!("ToolResult must map to tool_call_result");
        };
        assert_eq!(result.event, "tool_call_result");
        // The result must answer the id the start event announced.
        assert_eq!(result.data["tool_use_id"], id);
        assert_eq!(result.data["is_error"], true);
    }

    #[test]
    fn terminal_events_close_the_stream_and_silence_is_skipped() {
        let mut pending = VecDeque::new();
        assert!(matches!(
            map_agent_event(AgentEvent::ThinkingDelta("…".into()), &mut pending),
            Mapped::Continue(None)
        ));
        assert!(matches!(
            map_agent_event(AgentEvent::UsageUpdate {
                input_tokens: None,
                output_tokens: 1,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }, &mut pending),
            Mapped::Continue(None)
        ));

        let Mapped::Finished(done) = map_agent_event(AgentEvent::Done, &mut pending) else {
            panic!("Done must be terminal");
        };
        assert_eq!(done.event, "done");

        let Mapped::Finished(error) = map_agent_event(
            AgentEvent::Error("kaboom".to_owned()),
            &mut pending,
        ) else {
            panic!("Error must be terminal");
        };
        assert_eq!(error.event, "error");
        assert_eq!(error.data["message"], "kaboom");
    }

    #[test]
    fn long_tool_results_are_truncated() {
        let content = "x".repeat(TOOL_RESULT_PREVIEW_CHARS + 100);
        assert_eq!(
            truncate(&content, TOOL_RESULT_PREVIEW_CHARS).chars().count(),
            TOOL_RESULT_PREVIEW_CHARS + 1 // truncated body + ellipsis
        );
    }
}
