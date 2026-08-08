//! OpenAI Responses API wire protocol (`POST /v1/responses`).
//!
//! The Responses API is OpenAI's newer interface (replacing Chat Completions
//! as the recommended path for reasoning models). Its request shape differs
//! from Chat Completions in three main ways:
//!
//! - `instructions` (string) replaces the `system` role message.
//! - `input` is a flat list of typed items (`message`, `function_call`,
//!   `function_call_output`, `reasoning`) rather than role-tagged messages.
//! - `tools` are flat (`{type:"function", name, parameters}`) rather than
//!   nested under `function`.
//! - `reasoning: { effort: … }` replaces the top-level `reasoning_effort`.
//!
//! This adapter translates the canonical Anthropic-shaped IR into and out of
//! that shape.

use crate::types::errors::{AnthropicError, Result};
use crate::types::messages::{Message, MessageCreateParams, Role};
use crate::types::shared::{RequestId, Usage};
use crate::types::streaming::{ContentBlockDelta, MessageStreamEvent};
use crate::types::{ContentBlock, StopReason};
use crate::wire::openai::{
    OPENAI_FEATURES, function_descriptor, parse_json, reasoning_effort_value, translate_message,
    translate_tool_choice,
};
use crate::wire::{ProtocolFeatures, StreamState, WireProtocol, WireRequest};
use serde_json::{Value, json};

/// Endpoint path for the Responses API.
pub const ENDPOINT_PATH: &str = "/v1/responses";

/// Wire-protocol impl for the OpenAI Responses API.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAiResponsesWire;

impl OpenAiResponsesWire {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl WireProtocol for OpenAiResponsesWire {
    fn id(&self) -> &'static str {
        "openai_responses"
    }

    fn features(&self) -> ProtocolFeatures {
        OPENAI_FEATURES
    }

    fn encode_request(&self, params: &MessageCreateParams, streaming: bool) -> Result<WireRequest> {
        // Build the `input` array. Responses uses typed items, not
        // role-tagged messages, but role+content message items still appear
        // as `{"type":"message","role":…,"content":[…]}`.
        let mut input: Vec<Value> = Vec::new();
        for param in &params.messages {
            for msg in translate_message(param) {
                // `translate_message` returns Chat-shaped messages. Responses
                // expects message items to carry `type:"message"` and content
                // parts typed with `input_text`/`input_image` rather than
                // `text`/`image_url`. Tool messages (`role:"tool"`) map to
                // `function_call_output` items.
                let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
                if role == "tool" {
                    // Convert to a function_call_output item.
                    let call_id = msg
                        .get("tool_call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let output = msg
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": output,
                    }));
                    continue;
                }

                if let Some(tcs) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                    // Assistant message with tool calls → emit each as a
                    // `function_call` item.
                    for tc in tcs {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": tc["id"],
                            "name": tc["function"]["name"],
                            "arguments": tc["function"]["arguments"],
                        }));
                    }
                    // If there's also text content, emit it as a message item.
                    if let Some(content) = msg.get("content")
                        && !content.is_null() {
                            input.push(responses_message_item(role, content));
                        }
                    continue;
                }

                // Plain message item.
                let content = msg.get("content").cloned().unwrap_or(Value::Null);
                input.push(responses_message_item(role, &content));
            }
        }

        let mut body = json!({
            "model": params.model,
            "input": input,
            // Don't persist server-side state; the SDK manages conversation
            // history client-side.
            "store": false,
        });

        // `instructions` replaces the Chat Completions `system` message.
        if let Some(system) = &params.system
            && !system.is_empty() {
                body["instructions"] = json!(system);
            }

        if params.max_tokens > 0 {
            body["max_output_tokens"] = json!(params.max_tokens);
        }
        if let Some(t) = params.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(t) = params.top_p {
            body["top_p"] = json!(t);
        }
        if streaming {
            body["stream"] = json!(true);
        }

        // Tools: flat function descriptors.
        if let Some(tools) = &params.tools
            && !tools.is_empty() {
                let openai_tools: Vec<Value> = tools
                    .iter()
                    .map(|t| {
                        let f = function_descriptor(t);
                        // Flatten: Responses wants `name`/`parameters` at the
                        // top level alongside `type:"function"`.
                        let mut obj = serde_json::Map::new();
                        obj.insert("type".to_string(), json!("function"));
                        if let Some(obj_inner) = f.as_object() {
                            for (k, v) in obj_inner {
                                obj.insert(k.clone(), v.clone());
                            }
                        }
                        Value::Object(obj)
                    })
                    .collect();
                body["tools"] = Value::Array(openai_tools);
            }

        if let Some(choice) = &params.tool_choice {
            body["tool_choice"] = translate_tool_choice(choice, /*chat_form*/ false);
        }

        // Reasoning: nested `{effort:…}` on Responses.
        if let Some(effort) = reasoning_effort_value(params) {
            body["reasoning"] = json!({"effort": effort});
        }

        let body_bytes = serde_json::to_vec(&body).map_err(|e| AnthropicError::Connection {
            message: format!("failed to serialise OpenAI Responses request body: {e}"),
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

        let mut content: Vec<ContentBlock> = Vec::new();
        let mut stop_reason: Option<StopReason> = None;

        // Iterate the `output` array. Each item is typed:
        // `message` (with `content:[{type:"output_text",text}]`),
        // `function_call` ({call_id,name,arguments}),
        // `reasoning` ({summary:[{type:"summary_text",text}]}).
        if let Some(output) = value.get("output").and_then(|o| o.as_array()) {
            for item in output {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("message") => {
                        if let Some(parts) = item.get("content").and_then(|c| c.as_array()) {
                            for part in parts {
                                if part.get("type").and_then(|t| t.as_str()) == Some("output_text")
                                {
                                    let text = part
                                        .get("text")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if !text.is_empty() {
                                        content.push(ContentBlock::Text { text });
                                    }
                                }
                            }
                        }
                    }
                    Some("function_call") => {
                        let call_id = item
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments_str = item
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}");
                        let input: Value = serde_json::from_str(arguments_str).unwrap_or(json!({}));
                        content.push(ContentBlock::ToolUse {
                            id: call_id,
                            name,
                            input,
                        });
                        stop_reason = Some(StopReason::ToolUse);
                    }
                    Some("reasoning") => {
                        if let Some(summary) = item.get("summary").and_then(|s| s.as_array()) {
                            for s in summary {
                                if s.get("type").and_then(|t| t.as_str()) == Some("summary_text") {
                                    let text = s
                                        .get("text")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if !text.is_empty() {
                                        content.push(ContentBlock::Thinking {
                                            thinking: text,
                                            signature: String::new(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // If no explicit stop_reason was inferred from a function_call, check
        // the response status.
        if stop_reason.is_none() {
            stop_reason = match value.get("status").and_then(|s| s.as_str()) {
                Some("completed") => Some(StopReason::EndTurn),
                Some("incomplete") | Some("incomplete_output") => Some(StopReason::MaxTokens),
                _ => Some(StopReason::EndTurn),
            };
        }

        let usage = value.get("usage").map(|u| Usage {
            input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            cache_creation_input_tokens: u
                .get("input_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_u64()),
            cache_read_input_tokens: None,
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
        // The Responses API uses named SSE events: `response.created`,
        // `response.output_text.delta`, `response.function_call_arguments.delta`,
        // `response.completed`, etc.
        match event_type {
            "response.created" | "response.in_progress" => {
                if !state.started {
                    state.started = true;
                    let value: Value = serde_json::from_str(data).map_err(|e| {
                        AnthropicError::StreamError(format!(
                            "failed to parse response.created: {e}"
                        ))
                    })?;
                    let resp = &value["response"];
                    state.response_id = resp.get("id").and_then(|v| v.as_str()).map(str::to_string);
                    state.model = resp
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
                    return Ok(Some(MessageStreamEvent::MessageStart { message }));
                }
                Ok(None)
            }

            "response.output_item.added" => {
                // A new output item (message/function_call/reasoning) is
                // starting. For function_call items, open a ToolUse block.
                let value: Value = serde_json::from_str(data).map_err(|e| {
                    AnthropicError::StreamError(format!(
                        "failed to parse response.output_item.added: {e}"
                    ))
                })?;
                let item = &value["item"];
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("function_call") => {
                        let block_index = state.next_block_index;
                        state.next_block_index += 1;
                        let id = item
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let idx_key =
                            item.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        state.tool_calls.insert(
                            idx_key,
                            crate::wire::ToolCallSlot {
                                block_index,
                                id,
                                name: name.clone(),
                                block_started: true,
                                ..Default::default()
                            },
                        );
                        Ok(Some(MessageStreamEvent::ContentBlockStart {
                            content_block: ContentBlock::ToolUse {
                                id: state.tool_calls[&idx_key].id.clone(),
                                name,
                                input: json!({}),
                            },
                            index: block_index,
                        }))
                    }
                    Some("message") => {
                        // Open a text block at the next available index.
                        if !state.text_block_open {
                            state.text_block_open = true;
                            state.next_block_index = 0;
                        }
                        Ok(None)
                    }
                    _ => Ok(None),
                }
            }

            "response.output_text.delta" => {
                let value: Value = serde_json::from_str(data).map_err(|e| {
                    AnthropicError::StreamError(format!(
                        "failed to parse response.output_text.delta: {e}"
                    ))
                })?;
                let text = value.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                if text.is_empty() {
                    return Ok(None);
                }
                if !state.text_block_open {
                    state.text_block_open = true;
                    state.next_block_index = 0;
                    // We can only return one event; emit the start now and
                    // the delta will be carried by the next event.
                    return Ok(Some(MessageStreamEvent::ContentBlockStart {
                        content_block: ContentBlock::Text {
                            text: String::new(),
                        },
                        index: 0,
                    }));
                }
                Ok(Some(MessageStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::TextDelta {
                        text: text.to_string(),
                    },
                    index: 0,
                }))
            }

            "response.function_call_arguments.delta" => {
                let value: Value = serde_json::from_str(data).map_err(|e| {
                    AnthropicError::StreamError(format!(
                        "failed to parse response.function_call_arguments.delta: {e}"
                    ))
                })?;
                let partial = value.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                if partial.is_empty() {
                    return Ok(None);
                }
                let idx_key = value
                    .get("item_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let block_index = state
                    .tool_calls
                    .get(&idx_key)
                    .map(|s| s.block_index)
                    .unwrap_or(0);
                if let Some(slot) = state.tool_calls.get_mut(&idx_key) {
                    slot.arguments.push_str(partial);
                }
                Ok(Some(MessageStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::InputJsonDelta {
                        partial_json: partial.to_string(),
                    },
                    index: block_index,
                }))
            }

            "response.completed" => {
                // Final event: extract usage + stop_reason if present, then
                // terminate.
                if let Ok(value) = serde_json::from_str::<Value>(data) {
                    let resp = &value["response"];
                    if let Some(usage) = resp.get("usage") {
                        state.usage = Some(Usage {
                            input_tokens: usage
                                .get("input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0),
                            output_tokens: usage
                                .get("output_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0),
                            cache_creation_input_tokens: None,
                            cache_read_input_tokens: None,
                            server_tool_use: None,
                            service_tier: None,
                        });
                    }
                    state.stop_reason =
                        resp.get("status")
                            .and_then(|s| s.as_str())
                            .and_then(|s| match s {
                                "completed" => Some(StopReason::EndTurn),
                                "incomplete" | "incomplete_output" => Some(StopReason::MaxTokens),
                                _ => None,
                            });
                }
                Ok(Some(MessageStreamEvent::MessageStop))
            }

            // All other events (content_part.added, output_text.done,
            // output_item.done, response.failed, etc.) are skipped — the
            // canonical event stream doesn't need them.
            _ => Ok(None),
        }
    }
}

/// Convert a Chat-shaped content value into a Responses `message` input item.
///
/// Responses uses `input_text` / `input_image` content part types rather than
/// Chat's `text` / `image_url`.
fn responses_message_item(role: &str, content: &Value) -> Value {
    match content {
        Value::String(s) => json!({
            "type": "message",
            "role": role,
            "content": [{"type": "input_text", "text": s}],
        }),
        Value::Array(parts) => {
            let mapped: Vec<Value> = parts
                .iter()
                .map(|p| match p.get("type").and_then(|t| t.as_str()) {
                    Some("text") => json!({
                        "type": "input_text",
                        "text": p.get("text").cloned().unwrap_or(json!("")),
                    }),
                    Some("image_url") => json!({
                        "type": "input_image",
                        "image_url": p.get("image_url").cloned().unwrap_or(json!("")),
                    }),
                    _ => p.clone(),
                })
                .collect();
            json!({
                "type": "message",
                "role": role,
                "content": mapped,
            })
        }
        _ => json!({"type": "message", "role": role, "content": content.clone()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::messages::MessageCreateBuilder;
    use crate::types::tools::{ToolChoice, ToolDefinitionBuilder};

    #[test]
    fn responses_wire_encodes_basic_request() {
        let wire = OpenAiResponsesWire;
        let params = MessageCreateBuilder::new("gpt-4o", 1024)
            .system("Be helpful.")
            .user("Hello!")
            .build();
        let req = wire.encode_request(&params, false).unwrap();
        assert_eq!(req.endpoint_path, "/v1/responses");

        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["instructions"], "Be helpful.");
        assert_eq!(body["store"], false);
        // User message becomes a typed input item.
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][0]["content"][0]["text"], "Hello!");
        assert_eq!(body["max_output_tokens"], 1024);
    }

    #[test]
    fn responses_wire_emits_nested_reasoning() {
        let wire = OpenAiResponsesWire;
        let params = MessageCreateBuilder::new("o3", 4096)
            .user("think")
            .reasoning_effort(crate::types::ReasoningEffort::High)
            .build();
        let req = wire.encode_request(&params, false).unwrap();
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["reasoning"]["effort"], "high");
    }

    #[test]
    fn responses_wire_translates_tools_flat() {
        let wire = OpenAiResponsesWire;
        let tool = ToolDefinitionBuilder::new("search", "Search the web").build();
        let params = MessageCreateBuilder::new("gpt-4o", 1024)
            .user("find x")
            .tools(vec![tool])
            .tool_choice(ToolChoice::Any)
            .build();
        let req = wire.encode_request(&params, false).unwrap();
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "search");
        // No nested `function` wrapper on Responses.
        assert!(body["tools"][0].get("function").is_none());
        assert_eq!(body["tool_choice"], "required");
    }

    #[test]
    fn responses_wire_decodes_text_response() {
        let wire = OpenAiResponsesWire;
        let body = r#"{
            "id": "resp_1",
            "object": "response",
            "model": "gpt-4o",
            "status": "completed",
            "output": [
                {"type": "message", "role": "assistant", "content": [
                    {"type": "output_text", "text": "Hi there"}
                ]}
            ],
            "usage": {"input_tokens": 3, "output_tokens": 2}
        }"#;
        let msg = wire.decode_response(200, body, None).unwrap();
        assert_eq!(msg.id, "resp_1");
        assert_eq!(msg.role, Role::Assistant);
        match &msg.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Hi there"),
            other => panic!("expected text, got {other:?}"),
        }
        assert_eq!(msg.stop_reason, Some(StopReason::EndTurn));
        let usage = msg.usage.unwrap();
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.output_tokens, 2);
    }

    #[test]
    fn responses_wire_decodes_function_call() {
        let wire = OpenAiResponsesWire;
        let body = r#"{
            "id": "resp_2",
            "model": "gpt-4o",
            "status": "completed",
            "output": [
                {"type": "function_call", "call_id": "call_9", "name": "search", "arguments": "{\"q\":\"rust\"}"}
            ],
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }"#;
        let msg = wire.decode_response(200, body, None).unwrap();
        assert_eq!(msg.stop_reason, Some(StopReason::ToolUse));
        match &msg.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_9");
                assert_eq!(name, "search");
                assert_eq!(input["q"], "rust");
            }
            other => panic!("expected tool_use, got {other:?}"),
        }
    }

    #[test]
    fn responses_stream_emits_message_start_and_text_delta() {
        let wire = OpenAiResponsesWire;
        let mut state = StreamState::default();

        let created = r#"{"type":"response.created","response":{"id":"resp_x","model":"gpt-4o"}}"#;
        let ev = wire
            .adapt_sse_event("response.created", created, &mut state)
            .unwrap()
            .unwrap();
        assert!(matches!(ev, MessageStreamEvent::MessageStart { .. }));

        let delta = r#"{"type":"response.output_text.delta","delta":"Hello"}"#;
        // First delta triggers ContentBlockStart.
        let ev = wire
            .adapt_sse_event("response.output_text.delta", delta, &mut state)
            .unwrap()
            .unwrap();
        assert!(matches!(ev, MessageStreamEvent::ContentBlockStart { .. }));

        let delta2 = r#"{"type":"response.output_text.delta","delta":" world"}"#;
        let ev = wire
            .adapt_sse_event("response.output_text.delta", delta2, &mut state)
            .unwrap()
            .unwrap();
        match ev {
            MessageStreamEvent::ContentBlockDelta { delta, .. } => match delta {
                ContentBlockDelta::TextDelta { text } => assert_eq!(text, " world"),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }

        let completed = r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":1,"output_tokens":2}}}"#;
        let ev = wire
            .adapt_sse_event("response.completed", completed, &mut state)
            .unwrap()
            .unwrap();
        assert_eq!(ev, MessageStreamEvent::MessageStop);
    }
}
