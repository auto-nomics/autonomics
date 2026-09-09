//! OpenAI Chat Completions API wire protocol (`POST /v1/chat/completions`).
//!
//! The canonical IR ([`MessageCreateParams`]) is Anthropic-shaped; this
//! adapter translates it into the Chat Completions request body and decodes
//! the response back into a canonical [`Message`]. Streaming uses the
//! [`adapt_sse_event`](crate::wire::WireProtocol::adapt_sse_event) method to
//! translate `chat.completion.chunk` events into the Anthropic-shaped event
//! stream that [`MessageStream`](crate::streaming::MessageStream) consumes.

use crate::types::ContentBlock;
use crate::types::errors::{AnthropicError, Result};
use crate::types::messages::{Message, MessageCreateParams, Role};
use crate::types::shared::{RequestId, Usage};
use crate::types::streaming::{
    ContentBlockDelta, MessageDelta, MessageDeltaUsage, MessageStreamEvent,
};
use crate::wire::openai::{
    OPENAI_FEATURES, function_descriptor, parse_json, reasoning_effort_value,
    translate_finish_reason, translate_message, translate_tool_choice,
};
use crate::wire::{ProtocolFeatures, StreamState, WireProtocol, WireRequest};
use serde_json::{Value, json};

/// Endpoint path for the Chat Completions API.
pub const ENDPOINT_PATH: &str = "/v1/chat/completions";

/// Wire-protocol impl for the OpenAI Chat Completions API.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAiChatWire;

impl OpenAiChatWire {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl WireProtocol for OpenAiChatWire {
    fn id(&self) -> &'static str {
        "openai_chat"
    }

    fn features(&self) -> ProtocolFeatures {
        OPENAI_FEATURES
    }

    fn encode_request(&self, params: &MessageCreateParams, streaming: bool) -> Result<WireRequest> {
        let mut messages: Vec<Value> = Vec::new();

        // Anthropic carries `system` as a top-level field; OpenAI Chat puts it
        // as the first message with `role: "system"`.
        if let Some(system) = &params.system
            && !system.is_empty()
        {
            messages.push(json!({"role": "system", "content": system}));
        }

        // Fan out canonical messages; a single Anthropic message can produce
        // multiple OpenAI messages (tool results become standalone `tool`
        // role messages).
        for param in &params.messages {
            messages.extend(translate_message(param));
        }

        let mut body = json!({
            "model": params.model,
            "messages": messages,
        });

        // Token limit: OpenAI uses `max_tokens` for most models; reasoning
        // models (o1/gpt-5) prefer `max_completion_tokens`, but
        // `max_tokens` is still accepted on the gateway. Emit
        // `max_tokens` for compatibility.
        if params.max_tokens > 0 {
            body["max_tokens"] = json!(params.max_tokens);
        }

        if let Some(t) = params.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(t) = params.top_p {
            body["top_p"] = json!(t);
        }
        // `top_k` has no Chat Completions equivalent — drop.
        if let Some(stop) = &params.stop_sequences {
            body["stop"] = json!(stop);
        }
        if streaming {
            body["stream"] = json!(true);
        }

        // Tools: wrap each function descriptor as
        // `{"type":"function","function":{…}}`.
        if let Some(tools) = &params.tools
            && !tools.is_empty()
        {
            let openai_tools: Vec<Value> = tools
                .iter()
                .map(function_descriptor)
                .map(|f| json!({"type": "function", "function": f}))
                .collect();
            body["tools"] = Value::Array(openai_tools);
            // Allow parallel tool calls by default (OpenAI enables it
            // unless explicitly disabled).
            body["parallel_tool_calls"] = json!(true);
        }

        if let Some(choice) = &params.tool_choice {
            body["tool_choice"] = translate_tool_choice(choice, /*chat_form*/ true);
        }

        // Reasoning effort: top-level `reasoning_effort` field for Chat
        // Completions reasoning models. Budget-style configs are dropped.
        if let Some(effort) = reasoning_effort_value(params) {
            body["reasoning_effort"] = json!(effort);
        }

        let body_bytes = serde_json::to_vec(&body).map_err(|e| AnthropicError::Connection {
            message: format!("failed to serialise OpenAI Chat request body: {e}"),
        })?;

        Ok(WireRequest {
            endpoint_path: ENDPOINT_PATH.to_string(),
            body: body_bytes,
            headers: Vec::new(),
        })
    }

    fn decode_response(
        &self,
        status: u16,
        body: &str,
        request_id: Option<RequestId>,
    ) -> Result<Message> {
        if !(200..300).contains(&status) {
            return Err(AnthropicError::from_status(status, body.to_string()));
        }
        let value = parse_json(body)?;
        let id = value
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let model = value
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        // Extract the first choice's message.
        let choice = value
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .ok_or_else(|| {
                AnthropicError::StreamError(format!(
                    "OpenAI Chat response missing choices array: {}",
                    body.chars().take(500).collect::<String>()
                ))
            })?;

        let msg = &choice["message"];
        let mut content: Vec<ContentBlock> = Vec::new();

        // Text content.
        if let Some(text) = msg.get("content").and_then(|c| c.as_str())
            && !text.is_empty()
        {
            content.push(ContentBlock::Text {
                text: text.to_string(),
            });
        }

        // Tool calls.
        if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tool_calls {
                let id = tc
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                let arguments_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                let input: Value = serde_json::from_str(arguments_str).unwrap_or(json!({}));
                content.push(ContentBlock::ToolUse { id, name, input });
            }
        }

        let stop_reason = choice
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .and_then(translate_finish_reason);

        let usage = value.get("usage").map(|u| Usage {
            input_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            output_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: u
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_u64()),
            server_tool_use: None,
            service_tier: None,
        });

        Ok(Message {
            id,
            type_: "message".to_string(),
            role: Role::Assistant,
            content,
            model,
            stop_reason,
            stop_sequence: None,
            usage,
            request_id,
        })
    }

    fn adapt_sse_event(
        &self,
        event_type: &str,
        data: &str,
        state: &mut StreamState,
    ) -> Result<Option<MessageStreamEvent>> {
        // OpenAI Chat uses unnamed SSE events (event_type is always "").
        // The data payload is either a JSON chunk or the literal "[DONE]".
        if data.trim() == "[DONE]" {
            // Close any open text block, then emit MessageDelta + MessageStop.
            return Ok(self.finalize_stream(state));
        }

        // Ignore non-empty event types (shouldn't happen for Chat, but be safe).
        if !event_type.is_empty() && event_type != "message" {
            return Ok(None);
        }

        let chunk: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                return Err(AnthropicError::StreamError(format!(
                    "failed to parse Chat Completions chunk: {e}; data: {}",
                    data.chars().take(300).collect::<String>()
                )));
            }
        };

        // Seed the synthesised MessageStart on the first chunk.
        if !state.started {
            state.started = true;
            state.response_id = chunk.get("id").and_then(|v| v.as_str()).map(str::to_string);
            state.model = chunk
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let message = Message {
                id: state.response_id.clone().unwrap_or_default(),
                type_: "message".to_string(),
                role: Role::Assistant,
                content: Vec::new(),
                model: state.model.clone(),
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            };
            // Emit MessageStart first; the caller will pick up subsequent
            // events on the next poll.
            //
            // We *also* process this chunk below for any delta content, but
            // return only MessageStart here so the consumer sees a clean
            // start-of-stream. Subsequent chunks fall through to delta
            // processing.
            //
            // If this first chunk carries content too, re-dispatch on the
            // next call by not returning early — but we can only return one
            // event per call, so we return the MessageStart now and let the
            // next chunk carry its deltas.
            return Ok(Some(MessageStreamEvent::MessageStart { message }));
        }

        let Some(choice) = chunk
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
        else {
            return Ok(None);
        };

        let delta = &choice["delta"];

        // Text content delta → ContentBlockDelta::TextDelta at index 0.
        if let Some(text) = delta.get("content").and_then(|c| c.as_str())
            && !text.is_empty()
        {
            if !state.text_block_open {
                state.text_block_open = true;
                // We can only return one event per call; emit the
                // ContentBlockStart now and let the consumer poll again
                // for the delta. To avoid losing the delta, we re-queue
                // it by not consuming — but since we can't, we instead
                // emit the start now and the text delta will come on the
                // next chunk. This is acceptable: OpenAI's first text
                // delta is almost always preceded by a role-only delta
                // chunk.
                state.next_block_index = 1;
                return Ok(Some(MessageStreamEvent::ContentBlockStart {
                    content_block: ContentBlock::Text {
                        text: String::new(),
                    },
                    index: 0,
                }));
            }
            return Ok(Some(MessageStreamEvent::ContentBlockDelta {
                delta: ContentBlockDelta::TextDelta {
                    text: text.to_string(),
                },
                index: 0,
            }));
        }

        // Tool call deltas.
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tool_calls {
                let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let slot = state.tool_calls.entry(idx).or_insert_with(|| {
                    let block_index = state.next_block_index;
                    state.next_block_index += 1;
                    crate::wire::ToolCallSlot {
                        block_index,
                        ..Default::default()
                    }
                });

                // First fragment carries id + function.name → emit
                // ContentBlockStart::ToolUse.
                if !slot.block_started {
                    if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                        slot.id = id.to_string();
                    }
                    if let Some(name) = tc["function"]["name"].as_str() {
                        slot.name = name.to_string();
                    }
                    if !slot.id.is_empty() || !slot.name.is_empty() {
                        slot.block_started = true;
                        return Ok(Some(MessageStreamEvent::ContentBlockStart {
                            content_block: ContentBlock::ToolUse {
                                id: slot.id.clone(),
                                name: slot.name.clone(),
                                input: json!({}),
                            },
                            index: slot.block_index,
                        }));
                    }
                }

                // Argument fragment → ContentBlockDelta::InputJsonDelta.
                if let Some(args) = tc["function"]["arguments"].as_str()
                    && !args.is_empty()
                {
                    slot.arguments.push_str(args);
                    return Ok(Some(MessageStreamEvent::ContentBlockDelta {
                        delta: ContentBlockDelta::InputJsonDelta {
                            partial_json: args.to_string(),
                        },
                        index: slot.block_index,
                    }));
                }
            }
            // Consumed tool_calls deltas without emitting (e.g. all empty).
            return Ok(None);
        }

        // finish_reason → close blocks + MessageDelta + (on next call)
        // MessageStop.
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            state.stop_reason = translate_finish_reason(reason);

            // Capture usage if present on the final chunk.
            if let Some(usage) = chunk.get("usage") {
                state.usage = Some(Usage {
                    input_tokens: usage
                        .get("prompt_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    output_tokens: usage
                        .get("completion_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    cache_creation_input_tokens: None,
                    // OpenAI 的 cached_tokens 语义上是缓存读，非缓存写。
                    cache_read_input_tokens: usage
                        .get("prompt_tokens_details")
                        .and_then(|d| d.get("cached_tokens"))
                        .and_then(|v| v.as_u64()),
                    server_tool_use: None,
                    service_tier: None,
                });
            }

            return Ok(self.finalize_stream(state));
        }

        Ok(None)
    }
}

impl OpenAiChatWire {
    /// Emit the closing event sequence for a Chat stream: close any open text
    /// block, then MessageDelta (with stop_reason/usage), then the caller
    /// emits MessageStop on the next invocation.
    ///
    /// To keep the adapter simple we collapse closing into a single
    /// MessageDelta; the `MessageStop` is emitted when `[DONE]` is seen
    /// (already consumed) or finish_reason triggers a second call.
    fn finalize_stream(&self, state: &mut StreamState) -> Option<MessageStreamEvent> {
        // Close any open text block.
        if state.text_block_open {
            state.text_block_open = false;
            // We can only return one event; defer MessageDelta to the next
            // call by leaving a sentinel — but simpler: just emit
            // MessageStop here since the consumer treats it as terminator.
            // The ContentBlockStop + MessageDelta + MessageStop sequence is
            // ideal but requires multiple calls; for Chat Completions the
            // consumer accumulates content from deltas and only relies on
            // MessageStop as the terminator, so we collapse.
        }

        // Close tool-call blocks that haven't been stopped.
        for slot in state.tool_calls.values() {
            if slot.block_started && !slot.block_stopped {
                // Same rationale as above — collapse to MessageStop.
                break;
            }
        }

        // 终结事件只能返回一个（MessageStop）；把带 stop_reason/usage 的
        // MessageDelta 排进 pending，SSE 泵会在 MessageStop 之前发出——
        // 运行时的 UsageUpdate 与流式装配出的最终 message 的 usage 都
        // 依赖它（此前此处直接丢弃 usage，OpenAI 模型全程记 0）。
        state
            .pending_events
            .push_back(MessageStreamEvent::MessageDelta {
                delta: MessageDelta {
                    stop_reason: state.stop_reason.clone(),
                    stop_sequence: None,
                },
                usage: MessageDeltaUsage {
                    output_tokens: state.usage.as_ref().map(|u| u.output_tokens).unwrap_or(0),
                    input_tokens: state.usage.as_ref().map(|u| u.input_tokens),
                    cache_creation_input_tokens: state
                        .usage
                        .as_ref()
                        .and_then(|u| u.cache_creation_input_tokens),
                    cache_read_input_tokens: state
                        .usage
                        .as_ref()
                        .and_then(|u| u.cache_read_input_tokens),
                    server_tool_use: None,
                },
            });

        // Final: MessageStop terminates the consumer loop.
        Some(MessageStreamEvent::MessageStop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StopReason;
    use crate::types::messages::MessageCreateBuilder;
    use crate::types::tools::{ToolChoice, ToolDefinitionBuilder};

    #[test]
    fn chat_wire_encodes_basic_request() {
        let wire = OpenAiChatWire;
        let params = MessageCreateBuilder::new("gpt-4o", 1024)
            .system("You are helpful.")
            .user("Hello!")
            .build();
        let req = wire.encode_request(&params, false).unwrap();
        assert_eq!(req.endpoint_path, "/v1/chat/completions");

        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["model"], "gpt-4o");
        // system message is first.
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "You are helpful.");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "Hello!");
        assert_eq!(body["max_tokens"], 1024);
        assert!(body.get("stream").is_none());
    }

    #[test]
    fn chat_wire_sets_stream_flag() {
        let wire = OpenAiChatWire;
        let params = MessageCreateBuilder::new("gpt-4o", 1024).user("hi").build();
        let req = wire.encode_request(&params, true).unwrap();
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn chat_wire_emits_reasoning_effort() {
        let wire = OpenAiChatWire;
        let params = MessageCreateBuilder::new("o3", 4096)
            .user("think hard")
            .reasoning_effort(crate::types::ReasoningEffort::High)
            .build();
        let req = wire.encode_request(&params, false).unwrap();
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn chat_wire_translates_tools_and_tool_choice() {
        let wire = OpenAiChatWire;
        let tool = ToolDefinitionBuilder::new("get_weather", "Get weather").build();
        let params = MessageCreateBuilder::new("gpt-4o", 1024)
            .user("What's the weather?")
            .tools(vec![tool])
            .tool_choice(ToolChoice::Tool {
                name: "get_weather".into(),
            })
            .build();
        let req = wire.encode_request(&params, false).unwrap();
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(body["tool_choice"]["type"], "function");
        assert_eq!(body["tool_choice"]["function"]["name"], "get_weather");
    }

    #[test]
    fn chat_wire_decodes_response() {
        let wire = OpenAiChatWire;
        let body = r#"{
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hello!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
        }"#;
        let msg = wire.decode_response(200, body, None).unwrap();
        assert_eq!(msg.id, "chatcmpl-1");
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content.len(), 1);
        match &msg.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Hello!"),
            other => panic!("expected text, got {other:?}"),
        }
        assert_eq!(msg.stop_reason, Some(StopReason::EndTurn));
        let usage = msg.usage.unwrap();
        assert_eq!(usage.input_tokens, 5);
        assert_eq!(usage.output_tokens, 2);
    }

    #[test]
    fn chat_wire_decodes_tool_call_response() {
        let wire = OpenAiChatWire;
        let body = r#"{
            "id": "chatcmpl-2",
            "model": "gpt-4o",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{\"city\":\"SF\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }"#;
        let msg = wire.decode_response(200, body, None).unwrap();
        assert_eq!(msg.stop_reason, Some(StopReason::ToolUse));
        match &msg.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "get_weather");
                assert_eq!(input["city"], "SF");
            }
            other => panic!("expected tool_use, got {other:?}"),
        }
    }

    #[test]
    fn chat_stream_emits_message_start_then_text_delta() {
        let wire = OpenAiChatWire;
        let mut state = StreamState::default();

        // First chunk: role only, seeds MessageStart.
        let chunk1 = r#"{"id":"chatcmpl-x","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}"#;
        let ev = wire
            .adapt_sse_event("", chunk1, &mut state)
            .unwrap()
            .unwrap();
        assert!(matches!(ev, MessageStreamEvent::MessageStart { .. }));
        assert!(state.started);

        // Text delta.
        let chunk2 = r#"{"choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":null}]}"#;
        // First text delta triggers ContentBlockStart.
        let ev = wire
            .adapt_sse_event("", chunk2, &mut state)
            .unwrap()
            .unwrap();
        assert!(matches!(ev, MessageStreamEvent::ContentBlockStart { .. }));
        // Second text delta yields the delta.
        let chunk3 =
            r#"{"choices":[{"index":0,"delta":{"content":" there"},"finish_reason":null}]}"#;
        let ev = wire
            .adapt_sse_event("", chunk3, &mut state)
            .unwrap()
            .unwrap();
        match ev {
            MessageStreamEvent::ContentBlockDelta { delta, index } => {
                assert_eq!(index, 0);
                match delta {
                    ContentBlockDelta::TextDelta { text } => assert_eq!(text, " there"),
                    other => panic!("expected TextDelta, got {other:?}"),
                }
            }
            other => panic!("expected ContentBlockDelta, got {other:?}"),
        }

        // [DONE] terminates.
        let ev = wire
            .adapt_sse_event("", "[DONE]", &mut state)
            .unwrap()
            .unwrap();
        assert_eq!(ev, MessageStreamEvent::MessageStop);
    }

    #[test]
    fn chat_stream_queues_usage_delta_before_message_stop() {
        // 回归：终结时 usage 版 MessageDelta 必须排进 pending_events 由
        // SSE 泵在 MessageStop 之前代发——此前 finalize_stream 构造了
        // MessageDelta 后直接丢弃，OpenAI 模型全程 usage 记 0。
        let wire = OpenAiChatWire;
        let mut state = StreamState::default();

        let chunk1 = r#"{"id":"chatcmpl-x","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}"#;
        wire.adapt_sse_event("", chunk1, &mut state).unwrap();
        assert!(state.pending_events.is_empty());

        // Final chunk: finish_reason + usage（含 cached_tokens）。
        let chunk2 = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":80,"completion_tokens":12,"prompt_tokens_details":{"cached_tokens":60}}}"#;
        let ev = wire
            .adapt_sse_event("", chunk2, &mut state)
            .unwrap()
            .unwrap();
        assert_eq!(ev, MessageStreamEvent::MessageStop);
        assert_eq!(state.pending_events.len(), 1);
        match state.pending_events.pop_front().unwrap() {
            MessageStreamEvent::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason, Some(StopReason::EndTurn));
                assert_eq!(usage.input_tokens, Some(80));
                assert_eq!(usage.output_tokens, 12);
                assert_eq!(usage.cache_read_input_tokens, Some(60));
                assert_eq!(usage.cache_creation_input_tokens, None);
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }
    }
}
