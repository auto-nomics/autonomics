//! Ollama Chat API wire protocol (`POST /api/chat`).
//!
//! The SDK's canonical request shape is Anthropic-like. This adapter turns it
//! into Ollama's native chat format, including images, tools, thinking, and
//! newline-delimited JSON streaming.

use std::collections::HashMap;

use agentik_types::messages::{
    ContentBlockParam, ImageSource, Message, MessageContent, MessageCreateParams, MessageParam,
    Role,
};
use agentik_types::reasoning::{ReasoningConfig, ReasoningEffort, ThinkingKind};
use agentik_types::streaming::{
    ContentBlockDelta, MessageDelta, MessageDeltaUsage, MessageStreamEvent,
};
use agentik_types::tools::ToolChoice;
use agentik_types::{ContentBlock, StopReason, Usage};
use serde_json::{Value, json};

use crate::types::errors::{AnthropicError, Result};
use crate::types::shared::RequestId;
use crate::wire::{ProtocolFeatures, StreamState, ThinkingSupport, WireProtocol, WireRequest};

pub const ENDPOINT_PATH: &str = "/api/chat";

#[derive(Debug, Clone, Copy, Default)]
pub struct OllamaChatWire;

impl OllamaChatWire {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl WireProtocol for OllamaChatWire {
    fn id(&self) -> &'static str {
        "ollama_chat"
    }

    fn features(&self) -> ProtocolFeatures {
        ProtocolFeatures {
            thinking: ThinkingSupport::Off,
            tool_use: true,
            parallel_tool_calls: true,
            image_input: true,
            system_as_message: true,
            signed_thinking: false,
        }
    }

    fn stream_transport(&self) -> crate::wire::StreamTransport {
        crate::wire::StreamTransport::NewlineDelimitedJson
    }

    fn encode_request(&self, params: &MessageCreateParams, streaming: bool) -> Result<WireRequest> {
        let mut messages = Vec::new();
        if let Some(system) = params.system.as_deref().filter(|s| !s.is_empty()) {
            messages.push(json!({"role": "system", "content": system}));
        }

        let tool_names = tool_names_by_id(params);
        for param in &params.messages {
            translate_message(param, &tool_names, &mut messages)?;
        }

        let mut options = serde_json::Map::new();
        if params.max_tokens > 0 {
            options.insert("num_predict".into(), json!(params.max_tokens));
        }
        if let Some(value) = params.temperature {
            options.insert("temperature".into(), json!(value));
        }
        if let Some(value) = params.top_p {
            options.insert("top_p".into(), json!(value));
        }
        if let Some(value) = params.top_k {
            options.insert("top_k".into(), json!(value));
        }
        if let Some(stop) = params.stop_sequences.as_ref().filter(|s| !s.is_empty()) {
            options.insert("stop".into(), json!(stop));
        }

        let mut body = json!({
            "model": params.model,
            "messages": messages,
            "stream": streaming,
        });
        if !options.is_empty() {
            body["options"] = Value::Object(options);
        }

        if let Some(tools) = params.tools.as_ref().filter(|t| !t.is_empty()) {
            body["tools"] = Value::Array(
                tools
                    .iter()
                    .map(|tool| {
                        let parameters = serde_json::to_value(&tool.input_schema)
                            .unwrap_or_else(|_| json!({"type": "object", "properties": {}}));
                        json!({
                            "type": "function",
                            "function": {
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": parameters,
                            }
                        })
                    })
                    .collect(),
            );
        }

        if let Some(choice) = &params.tool_choice {
            match choice {
                ToolChoice::Tool { name } => {
                    body["tool_choice"] = json!({"type": "function", "function": {"name": name}});
                }
                ToolChoice::Any => body["tool_choice"] = json!("required"),
                ToolChoice::Auto => {}
            }
        }

        if let Some(think) = requested_thinking(params) {
            body["think"] = json!(think);
        }

        let bytes = serde_json::to_vec(&body).map_err(|e| AnthropicError::Connection {
            message: format!("failed to serialise Ollama Chat request body: {e}"),
        })?;
        Ok(WireRequest {
            endpoint_path: ENDPOINT_PATH.to_string(),
            body: bytes,
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
        let model = value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string);
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("created_at")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let content = decode_content(&value["message"]);
        let stop_reason = stop_reason(&value, !content.is_empty());
        let usage = decode_usage(&value);

        Ok(Message {
            id,
            type_: "message".to_string(),
            role: Role::Assistant,
            content,
            model,
            stop_reason: Some(stop_reason),
            stop_sequence: None,
            usage: Some(usage),
            request_id,
        })
    }

    fn adapt_sse_event(
        &self,
        _event_type: &str,
        data: &str,
        state: &mut StreamState,
    ) -> Result<Option<MessageStreamEvent>> {
        let chunk = parse_json(data)?;
        let done = chunk.get("done").and_then(Value::as_bool) == Some(true);
        if !state.started {
            state.started = true;
            state.response_id = chunk
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| chunk.get("created_at").and_then(Value::as_str))
                .map(str::to_string);
            state.model = chunk
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string);
            let start = MessageStreamEvent::MessageStart {
                message: Message {
                    id: state.response_id.clone().unwrap_or_default(),
                    type_: "message".to_string(),
                    role: Role::Assistant,
                    content: Vec::new(),
                    model: state.model.clone(),
                    stop_reason: None,
                    stop_sequence: None,
                    usage: None,
                    request_id: None,
                },
            };
            if done {
                state.pending_events.push_back(start);
                push_chunk_content(&chunk["message"], state)?;
                state.usage = Some(decode_usage(&chunk));
                state.stop_reason = Some(stop_reason(&chunk, false));
                finish_stream(state);
                return Ok(state.pending_events.pop_back());
            }

            push_chunk_content(&chunk["message"], state)?;
            state.deferred_events = std::mem::take(&mut state.pending_events);
            return Ok(Some(start));
        }

        let deferred = std::mem::take(&mut state.deferred_events);
        state.pending_events.extend(deferred);
        push_chunk_content(&chunk["message"], state)?;
        if done {
            state.usage = Some(decode_usage(&chunk));
            state.stop_reason = Some(stop_reason(&chunk, false));
            finish_stream(state);
        }

        let last = state.pending_events.pop_back();
        Ok(last)
    }
}

fn parse_json(body: &str) -> Result<Value> {
    serde_json::from_str(body).map_err(|e| {
        AnthropicError::StreamError(format!(
            "failed to parse Ollama response as JSON: {e}; body: {}",
            body.chars().take(500).collect::<String>()
        ))
    })
}

fn tool_names_by_id(params: &MessageCreateParams) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for message in &params.messages {
        if let MessageContent::Blocks(blocks) = &message.content {
            for block in blocks {
                if let ContentBlockParam::ToolUse { id, name, .. } = block {
                    names.insert(id.clone(), name.clone());
                }
            }
        }
    }
    names
}

fn translate_message(
    param: &MessageParam,
    tool_names: &HashMap<String, String>,
    out: &mut Vec<Value>,
) -> Result<()> {
    let role = match param.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    match &param.content {
        MessageContent::Text(text) => out.push(json!({"role": role, "content": text})),
        MessageContent::Blocks(blocks) => {
            let mut text = Vec::new();
            let mut images = Vec::new();
            let mut tool_calls = Vec::new();
            let mut tool_results = Vec::new();

            for block in blocks {
                match block {
                    ContentBlockParam::Text { text: value } => text.push(value.clone()),
                    ContentBlockParam::Thinking { .. } => {}
                    ContentBlockParam::Image { source } => match source {
                        ImageSource::Base64 { data, .. } => images.push(json!(data)),
                        ImageSource::Url { .. } => {
                            return Err(AnthropicError::Connection {
                                message: "Ollama image input requires base64 data".to_string(),
                            });
                        }
                    },
                    ContentBlockParam::ToolUse { id, name, input } => {
                        tool_calls.push(json!({
                            "id": id,
                            "function": {"name": name, "arguments": input},
                        }));
                    }
                    ContentBlockParam::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        let mut message = json!({
                            "role": "tool",
                            "content": content.clone().unwrap_or_default(),
                        });
                        if let Some(name) = tool_names.get(tool_use_id) {
                            message["tool_name"] = json!(name);
                        }
                        tool_results.push(message);
                    }
                }
            }

            if !text.is_empty() || !images.is_empty() || !tool_calls.is_empty() {
                let mut message = serde_json::Map::new();
                message.insert("role".into(), json!(role));
                message.insert("content".into(), json!(text.join("\n")));
                if !images.is_empty() {
                    message.insert("images".into(), Value::Array(images));
                }
                if !tool_calls.is_empty() {
                    message.insert("tool_calls".into(), Value::Array(tool_calls));
                }
                out.push(Value::Object(message));
            }
            out.extend(tool_results);
        }
    }
    Ok(())
}

fn requested_thinking(params: &MessageCreateParams) -> Option<bool> {
    if let Some(thinking) = &params.thinking {
        return Some(matches!(thinking.kind, ThinkingKind::Enabled));
    }
    match &params.reasoning {
        Some(ReasoningConfig::Budget { .. }) => Some(true),
        Some(ReasoningConfig::Effort(ReasoningEffort::None)) => Some(false),
        Some(ReasoningConfig::Effort(_)) => Some(true),
        None => None,
    }
}

fn decode_content(message: &Value) -> Vec<ContentBlock> {
    let mut content = Vec::new();
    if let Some(thinking) = message
        .get("thinking")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        content.push(ContentBlock::Thinking {
            thinking: thinking.to_string(),
            signature: String::new(),
        });
    }
    if let Some(text) = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        content.push(ContentBlock::Text {
            text: text.to_string(),
        });
    }
    for (index, call) in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let function = &call["function"];
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let input = function
            .get("arguments")
            .cloned()
            .filter(|value| !value.is_null())
            .unwrap_or_else(|| json!({}));
        content.push(ContentBlock::ToolUse {
            id: call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("tool_{index}")),
            name,
            input,
        });
    }
    content
}

fn decode_usage(value: &Value) -> Usage {
    Usage {
        input_tokens: value
            .get("prompt_eval_count")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value.get("eval_count").and_then(Value::as_u64).unwrap_or(0),
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
        server_tool_use: None,
        service_tier: None,
    }
}

fn stop_reason(value: &Value, has_content: bool) -> StopReason {
    if value["message"]
        .get("tool_calls")
        .and_then(Value::as_array)
        .is_some_and(|calls| !calls.is_empty())
    {
        return StopReason::ToolUse;
    }
    match value.get("done_reason").and_then(Value::as_str) {
        Some("length") | Some("max_tokens") => StopReason::MaxTokens,
        _ if has_content => StopReason::EndTurn,
        _ => StopReason::EndTurn,
    }
}

fn push_chunk_content(message: &Value, state: &mut StreamState) -> Result<()> {
    if let Some(thinking) = message
        .get("thinking")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let index = state.next_block_index;
        state.next_block_index += 1;
        state.blocks.push(ContentBlock::Thinking {
            thinking: String::new(),
            signature: String::new(),
        });
        state
            .pending_events
            .push_back(content_start::thinking(index));
        state.pending_events.push_back(delta(
            index,
            ContentBlockDelta::ThinkingDelta {
                thinking: thinking.to_string(),
            },
        ));
    }

    if let Some(text) = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        if !state.text_block_open {
            state.text_block_open = true;
            state.text_block_index = state.next_block_index;
            state.next_block_index += 1;
            state.blocks.push(ContentBlock::Text {
                text: String::new(),
            });
            state
                .pending_events
                .push_back(content_start::text(state.text_block_index));
        }
        let index = state.text_block_index;
        state.pending_events.push_back(delta(
            index,
            ContentBlockDelta::TextDelta {
                text: text.to_string(),
            },
        ));
    }

    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let function = &call["function"];
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let arguments = function
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let serialized = serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".into());
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("tool_{}", state.tool_calls.len()));
            let slot = crate::wire::ToolCallSlot {
                block_index: state.next_block_index,
                id,
                name,
                arguments: serialized.clone(),
                block_started: true,
                block_stopped: false,
            };
            let index = slot.block_index;
            let start_block = ContentBlock::ToolUse {
                id: slot.id.clone(),
                name: slot.name.clone(),
                input: json!({}),
            };
            state.next_block_index += 1;
            state.blocks.push(start_block.clone());
            state.tool_calls.insert(slot_index(state), slot);
            state
                .pending_events
                .push_back(content_start::tool(index, start_block));
            state.pending_events.push_back(delta(
                index,
                ContentBlockDelta::InputJsonDelta {
                    partial_json: serialized,
                },
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn finish_stream(state: &mut StreamState) {
    if state.text_block_open {
        state.text_block_open = false;
        state
            .pending_events
            .push_back(MessageStreamEvent::ContentBlockStop {
                index: state.text_block_index,
            });
    }
    for slot in state.tool_calls.values_mut() {
        if slot.block_started && !slot.block_stopped {
            slot.block_stopped = true;
            state
                .pending_events
                .push_back(MessageStreamEvent::ContentBlockStop {
                    index: slot.block_index,
                });
        }
    }
    state
        .pending_events
        .push_back(MessageStreamEvent::MessageDelta {
            delta: MessageDelta {
                stop_reason: state.stop_reason.clone(),
                stop_sequence: None,
            },
            usage: MessageDeltaUsage {
                output_tokens: state
                    .usage
                    .as_ref()
                    .map(|usage| usage.output_tokens)
                    .unwrap_or(0),
                input_tokens: state.usage.as_ref().map(|usage| usage.input_tokens),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                server_tool_use: None,
            },
        });
    state
        .pending_events
        .push_back(MessageStreamEvent::MessageStop);
}

fn slot_index(state: &StreamState) -> u32 {
    u32::try_from(state.tool_calls.len()).unwrap_or(0)
}

mod content_start {
    use super::*;

    pub fn text(index: usize) -> MessageStreamEvent {
        MessageStreamEvent::ContentBlockStart {
            content_block: ContentBlock::Text {
                text: String::new(),
            },
            index,
        }
    }

    pub fn thinking(index: usize) -> MessageStreamEvent {
        MessageStreamEvent::ContentBlockStart {
            content_block: ContentBlock::Thinking {
                thinking: String::new(),
                signature: String::new(),
            },
            index,
        }
    }

    pub fn tool(index: usize, block: ContentBlock) -> MessageStreamEvent {
        MessageStreamEvent::ContentBlockStart {
            content_block: block,
            index,
        }
    }
}

fn delta(index: usize, delta: ContentBlockDelta) -> MessageStreamEvent {
    MessageStreamEvent::ContentBlockDelta { delta, index }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::messages::MessageCreateBuilder;
    use agentik_types::messages::ContentBlockParam;
    use agentik_types::reasoning::ThinkingConfig;
    use agentik_types::tools::{ToolChoice, ToolDefinition, ToolInputSchema};
    use serde_json::Map;

    fn tool() -> ToolDefinition {
        ToolDefinition {
            name: "get_weather".into(),
            description: "Get weather".into(),
            input_schema: ToolInputSchema {
                schema_type: "object".into(),
                properties: Map::new(),
                required: Vec::new(),
                additional: Map::new(),
            },
        }
    }

    #[test]
    fn encodes_native_chat_tools_images_and_history() {
        let params = MessageCreateBuilder::new("llama3.2:latest", 512)
            .system("be concise")
            .user(vec![
                ContentBlockParam::text("look"),
                ContentBlockParam::image_base64("image/png", "aW1hZ2U="),
            ])
            .assistant(vec![ContentBlockParam::ToolUse {
                id: "call-1".into(),
                name: "get_weather".into(),
                input: json!({"city": "Shanghai"}),
            }])
            .user(vec![ContentBlockParam::ToolResult {
                tool_use_id: "call-1".into(),
                content: Some("sunny".into()),
                is_error: Some(false),
            }])
            .tools(vec![tool()])
            .tool_choice(ToolChoice::Tool {
                name: "get_weather".into(),
            })
            .thinking(ThinkingConfig::enabled(256))
            .build();

        let request = OllamaChatWire.encode_request(&params, true).unwrap();
        assert_eq!(request.endpoint_path, "/api/chat");
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "llama3.2:latest");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["images"][0], "aW1hZ2U=");
        assert_eq!(
            body["messages"][2]["tool_calls"][0]["function"]["name"],
            "get_weather"
        );
        assert_eq!(body["messages"][3]["tool_name"], "get_weather");
        assert_eq!(body["options"]["num_predict"], 512);
        assert_eq!(body["think"], true);
        assert_eq!(body["tools"][0]["function"]["name"], "get_weather");
    }

    #[test]
    fn decodes_thinking_tools_and_usage() {
        let body = r#"{
            "id": "abc",
            "model": "qwen3:4b",
            "message": {
                "thinking": "thinking",
                "content": "answer",
                "tool_calls": [{
                    "function": {"name": "get_weather", "arguments": {"city": "Shanghai"}}
                }]
            },
            "done_reason": "stop",
            "prompt_eval_count": 7,
            "eval_count": 3
        }"#;
        let message = OllamaChatWire.decode_response(200, body, None).unwrap();
        assert_eq!(message.content.len(), 3);
        assert_eq!(message.stop_reason, Some(StopReason::ToolUse));
        let usage = message.usage.unwrap();
        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.output_tokens, 3);
    }

    fn collect(
        wire: OllamaChatWire,
        data: &str,
        state: &mut StreamState,
    ) -> Vec<MessageStreamEvent> {
        let main = wire.adapt_sse_event("", data, state).unwrap();
        let mut events: Vec<_> = state.pending_events.drain(..).collect();
        if let Some(event) = main {
            events.push(event);
        }
        events
    }

    fn event_name(event: &MessageStreamEvent) -> &'static str {
        match event {
            MessageStreamEvent::MessageStart { .. } => "start",
            MessageStreamEvent::MessageDelta { .. } => "delta",
            MessageStreamEvent::MessageStop => "stop",
            MessageStreamEvent::ContentBlockStart { .. } => "block-start",
            MessageStreamEvent::ContentBlockDelta { .. } => "block-delta",
            MessageStreamEvent::ContentBlockStop { .. } => "block-stop",
        }
    }

    #[test]
    fn stream_preserves_first_chunk_text() {
        let wire = OllamaChatWire;
        let mut state = StreamState::default();
        let events = collect(
            wire,
            r#"{"model":"llama3.2","created_at":"now","message":{"content":"hi"},"done":true,"prompt_eval_count":2,"eval_count":1}"#,
            &mut state,
        );
        let names: Vec<_> = events.iter().map(event_name).collect();
        assert_eq!(
            names,
            [
                "start",
                "block-start",
                "block-delta",
                "block-stop",
                "delta",
                "stop"
            ]
        );
    }

    #[test]
    fn ollama_advertises_ndjson_streaming() {
        assert_eq!(
            OllamaChatWire.stream_transport(),
            crate::wire::StreamTransport::NewlineDelimitedJson
        );
    }

    #[test]
    fn stream_emits_tool_block_and_usage() {
        let wire = OllamaChatWire;
        let mut state = StreamState::default();
        let mut events = Vec::new();
        events.extend(collect(
            wire,
            r#"{"model":"qwen3","message":{"role":"assistant"},"done":false}"#,
            &mut state,
        ));
        events.extend(collect(
            wire,
            r#"{"model":"qwen3","message":{"tool_calls":[{"function":{"name":"get_weather","arguments":{"city":"Shanghai"}}}]},"done":false}"#,
            &mut state,
        ));
        events.extend(collect(
            wire,
            r#"{"model":"qwen3","message":{},"done":true,"prompt_eval_count":4,"eval_count":2,"done_reason":"stop"}"#,
            &mut state,
        ));

        let names: Vec<_> = events.iter().map(event_name).collect();
        assert_eq!(
            names,
            [
                "start",
                "block-start",
                "block-delta",
                "block-stop",
                "delta",
                "stop"
            ]
        );
    }
}
