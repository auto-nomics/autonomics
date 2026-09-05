//! SSE bridge exposing the agentik chat engine to the web frontend.
//!
//! The TUI already chats through agentik; this module is the same engine's
//! HTTP entrance. Each request builds an ephemeral [`Agent`] wired to the
//! process-wide bibliography tools (`bib_all_registrations`) and the model
//! slot owned by the TUI — the browser never sees an API key. The frontend
//! persists transcripts itself (`/api/v1/bib/chat`), so the agent runs
//! without storage and dies with the stream.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use agentik_core::agent::{Agent, InternalEvent};
use agentik_sdk::model::Model;
use agentik_sdk::types::AgentEvent;
use agentik_types::{Message, Role};
use arc_swap::ArcSwapOption;
use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::post;
use axum::{Router, response::IntoResponse};
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How often the stream emits an explicit `ping` event. The frontend resets
/// its 30 s idle timer on every *parsed event* — a `:`-comment keep-alive is
/// invisible to its parser, so the ping must be a named event.
const PING_INTERVAL: Duration = Duration::from_secs(10);

/// Tool-result previews are capped so a chatty `lit_search` cannot flood the
/// SSE stream; the frontend renders previews, not full payloads.
const TOOL_RESULT_PREVIEW_CHARS: usize = 500;

#[derive(Clone)]
pub(crate) struct AgentState {
    pub(crate) shared: bib_base::BibShared,
    /// The TUI's active-model slot, shared live: swapping the model in the
    /// TUI affects the next web request without restarting anything.
    pub(crate) model: Arc<ArcSwapOption<Model>>,
}

pub(crate) fn router(state: AgentState) -> Router {
    Router::new()
        .route("/chat", post(chat))
        .with_state(state)
}

/// jayread's agent chat request. `model_config`, `tools` and the API key
/// inside them are accepted but ignored: the server owns the model (TUI
/// config) and the tool set (bibliography tools) by design.
#[derive(Deserialize)]
struct ChatRequest {
    message: String,
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    messages: Vec<HistoryMessage>,
    #[serde(default)]
    system_prompt: Option<String>,
}

#[derive(Deserialize)]
struct HistoryMessage {
    role: String,
    content: String,
}

fn error(status: StatusCode, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({ "error": message })),
    )
        .into_response()
}

async fn chat(State(state): State<AgentState>, Json(request): Json<ChatRequest>) -> axum::response::Response {
    let message = request.message.trim().to_owned();
    if message.is_empty() {
        return error(StatusCode::BAD_REQUEST, "message must not be empty");
    }

    // Checked up front so the UI gets a real status code instead of an SSE
    // stream that immediately errors.
    if state.model.load().as_ref().is_none() {
        return error(StatusCode::SERVICE_UNAVAILABLE, "no model configured");
    }

    let history: Vec<Message> = request
        .messages
        .iter()
        // The system prompt rides in its own field; the transcript is plain
        // user/assistant turns.
        .filter(|entry| matches!(entry.role.as_str(), "user" | "assistant"))
        .map(|entry| Message {
            id: Uuid::new_v4().to_string(),
            type_: "message".to_owned(),
            role: match entry.role.as_str() {
                "user" => Role::User,
                _ => Role::Assistant,
            },
            content: vec![agentik_types::ContentBlock::Text {
                text: entry.content.clone(),
            }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        })
        .collect();

    let (event_tx, event_rx) = unbounded_channel::<AgentEvent>();
    let cancel = CancellationToken::new();

    let shared = state.shared.clone();
    let identity = identity_for(request.agent_type.as_deref());
    let system_section = request.system_prompt.clone();
    let model_slot = state.model.clone();

    let mut builder = Agent::builder()
        .with_model(model_slot)
        .with_agent_event_tx(event_tx)
        .with_tools(bib_base::bib_all_registrations(
            shared.bib.clone(),
            shared.gateway.clone(),
            Some(shared.europe_pmc.clone()),
        ))
        .with_initial_messages(history)
        .with_cancel_token(cancel.clone())
        .with_system_prompt_identity(identity);
    if let Some(section) = system_section {
        // The paperReader panel ships its paper context here; the frontend is
        // the only place that knows which paper is open.
        builder = builder.with_system_prompt_section(section);
    }

    let agent = match builder.build().await {
        Ok(agent) => agent,
        Err(err) => {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("agent build failed: {err}"))
        }
    };

    let internal_tx = agent.internal_event_tx();
    let task: JoinHandle<()> = tokio::spawn(async move {
        // Consumes the agent's internal event receiver; returns once the
        // turn completes and a `Shutdown` arrives (or the token is cancelled).
        let mut agent = agent;
        agent.run().await;
    });
    let guard = AgentGuard {
        internal_tx: internal_tx.clone(),
        cancel: cancel.clone(),
        task,
    };

    // Drive the turn: `message` is the new user input, history was seeded
    // via `with_initial_messages`.
    let injected = internal_tx.send(InternalEvent::MessageInject {
        content: vec![agentik_types::ContentBlock::Text { text: message }],
        from_user: true,
        delegation_id: None,
    });
    if injected.is_err() {
        cancel.cancel();
        return error(StatusCode::INTERNAL_SERVER_ERROR, "agent task failed to start");
    }

    let mut ping = tokio::time::interval(PING_INTERVAL);
    // `interval` fires its first tick immediately; consume it so the stream
    // opens with real content (or silence), not a synthetic ping.
    ping.tick().await;

    let stream = futures::stream::unfold(
        StreamState {
            events: event_rx,
            ping,
            pending_tools: VecDeque::new(),
            finished: false,
            guard,
        },
        |mut state| async move {
            if state.finished {
                return None;
            }

            // Loop until an event worth emitting arrives: agentik produces
            // many events the frontend has no UI for, and returning `None`
            // here would close the stream prematurely.
            loop {
                let sse_event = tokio::select! {
                    maybe_event = state.events.recv() => match maybe_event {
                        Some(event) => match map_agent_event(event, &mut state.pending_tools) {
                            Mapped::Continue(Some(frame)) => frame_to_event(frame),
                            Mapped::Continue(None) => continue,
                            Mapped::Finished(frame) => {
                                state.finished = true;
                                state.shutdown();
                                frame_to_event(frame)
                            }
                        },
                        // The run loop exited without a terminal event (crash or
                        // cancellation); end the stream cleanly so the client
                        // does not hang on its idle timer.
                        None => {
                            state.finished = true;
                            state.shutdown();
                            sse("done", json!({}))
                        }
                    },
                    _ = state.ping.tick() => sse("ping", json!({})),
                };

                return Some((Ok::<_, std::convert::Infallible>(sse_event), state));
            }
        },
    );

    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response();
    // Belt-and-braces alongside KeepAlive: an explicit no-cache so dev
    // proxies do not buffer the stream.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    response
}

enum Mapped {
    /// Forward an SSE event, or stay silent for events the frontend does not
    /// know about.
    Continue(Option<Frame>),
    /// Terminal: emit this event and close the stream.
    Finished(Frame),
}

/// A wire frame in the frontend's SSE protocol, kept as structured data so
/// the mapping is unit-testable (axum `Event` cannot be rendered back to
/// text).
struct Frame {
    event: &'static str,
    data: serde_json::Value,
}

/// Build an SSE frame with a JSON data payload (`Event::data` takes a string,
/// not a `Value`).
pub(crate) fn sse(event: &'static str, data: impl serde::Serialize) -> Event {
    Event::default()
        .event(event)
        .data(serde_json::to_string(&data).expect("SSE payloads serialize"))
}

fn frame_to_event(frame: Frame) -> Event {
    sse(frame.event, frame.data)
}

/// Translate an agentik [`AgentEvent`] into the frontend's SSE protocol:
/// `text_delta` / `tool_call_start` / `tool_call_result` / `done` / `error`.
fn map_agent_event(event: AgentEvent, pending_tools: &mut VecDeque<String>) -> Mapped {
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

/// Stops the spawned agent task when the SSE response goes away — normally
/// after the terminal event, or on client abort (the response future is
/// dropped mid-poll, running this `Drop`).
struct AgentGuard {
    internal_tx: UnboundedSender<InternalEvent>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl Drop for AgentGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        let _ = self.internal_tx.send(InternalEvent::Shutdown);
        // `run()` exits on Shutdown; the abort is a belt-and-braces net for
        // the case where that event is lost. There is no storage to corrupt.
        self.task.abort();
    }
}

struct StreamState {
    events: UnboundedReceiver<AgentEvent>,
    ping: tokio::time::Interval,
    pending_tools: VecDeque<String>,
    finished: bool,
    guard: AgentGuard,
}

impl StreamState {
    fn shutdown(&mut self) {
        self.guard.cancel.cancel();
        let _ = self.guard.internal_tx.send(InternalEvent::Shutdown);
    }
}

/// Per-panel personas mirroring the frontend's `agent_type` hint.
fn identity_for(agent_type: Option<&str>) -> String {
    match agent_type {
        Some("paperReader") => "Autonomics 阅读助手（Web）：围绕用户当前打开的文献回答与讨论，必要时可用文献工具检索补充资料。".to_owned(),
        Some("screening") => "Autonomics 筛查助手（Web）：帮助用户快速判断文献是否纳入，给出简明依据。".to_owned(),
        _ => "Autonomics 文献库助手（Web）：帮用户检索、管理与研读文献，可用 lit_search / lit_fetch 检索 PubMed、arXiv、OpenAlex、Crossref、Semantic Scholar，用 bib_save 保存文献到本地文献库。".to_owned(),
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
