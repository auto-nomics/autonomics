//! Shared helpers for the OpenAI Chat Completions and Responses wire
//! adapters.
//!
//! Both OpenAI protocols share the same tool/function encoding and the same
//! reasoning-effort vocabulary; only the message-list shape and the SSE event
//! vocabulary differ. The per-protocol modules ([`chat`] and [`responses`])
//! reuse the helpers defined here.

pub mod chat;
pub mod responses;

pub use chat::OpenAiChatWire;
pub use responses::OpenAiResponsesWire;

use crate::types::ReasoningConfig;
use crate::types::errors::{AnthropicError, Result};
use crate::types::messages::{
    ContentBlockParam, ImageSource, MessageContent, MessageCreateParams, MessageParam, Role,
};
use crate::types::tools::{ToolChoice, ToolDefinition};
use serde_json::{Value, json};

/// Common capability set for OpenAI-family protocols.
pub(crate) const OPENAI_FEATURES: crate::wire::ProtocolFeatures = crate::wire::ProtocolFeatures {
    thinking: crate::wire::ThinkingSupport::OpenaiEffort,
    tool_use: true,
    parallel_tool_calls: true,
    image_input: true,
    system_as_message: true,
    signed_thinking: false,
};

/// Translate an Anthropic-shaped [`ToolDefinition`] into the OpenAI function
/// tool descriptor.
///
/// For Chat Completions this is wrapped as
/// `{"type":"function","function":{…}}`; for Responses the same fields sit at
/// the top level (`{"type":"function","name":…,"parameters":…}`). The caller
/// wraps accordingly.
pub(crate) fn function_descriptor(tool: &ToolDefinition) -> Value {
    // Re-serialise the schema to preserve the `type`/`properties`/`required`
    // fields plus any `additional` flatten entries.
    let parameters = serde_json::to_value(&tool.input_schema)
        .unwrap_or_else(|_| json!({"type": "object", "properties": {}}));
    json!({
        "name": tool.name,
        "description": tool.description,
        "parameters": parameters,
    })
}

/// Translate an Anthropic-shaped [`ToolChoice`] into the OpenAI value used by
/// both Chat and Responses (`"auto"` / `"required"` / `{"type":"function",
/// "function"/"name": …}`).
///
/// `chat_form = true` nests the name under `"function"` (Chat Completions);
/// `false` puts it at the top level (Responses).
pub(crate) fn translate_tool_choice(choice: &ToolChoice, chat_form: bool) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Any => json!("required"),
        ToolChoice::Tool { name } => {
            if chat_form {
                json!({"type": "function", "function": {"name": name}})
            } else {
                json!({"type": "function", "name": name})
            }
        }
    }
}

/// Emit a `[DONE]`-aware stop translation: returns the OpenAI `stop_reason`
/// string mapped to the Anthropic [`StopReason`](crate::types::StopReason).
pub(crate) fn translate_finish_reason(reason: &str) -> Option<crate::types::StopReason> {
    use crate::types::StopReason;
    match reason {
        "stop" | "content_filter" => Some(StopReason::EndTurn),
        "length" => Some(StopReason::MaxTokens),
        "tool_calls" | "function_call" => Some(StopReason::ToolUse),
        _ => None,
    }
}

/// Build the OpenAI reasoning-effort field value from a canonical
/// [`ReasoningConfig`], if any. Returns the lowercase effort string.
pub(crate) fn reasoning_effort_value(params: &MessageCreateParams) -> Option<&'static str> {
    let cfg = params.reasoning.as_ref()?;
    match cfg {
        ReasoningConfig::Effort(effort) => Some(match effort {
            crate::types::ReasoningEffort::None => "none",
            crate::types::ReasoningEffort::Minimal => "minimal",
            crate::types::ReasoningEffort::Low => "low",
            crate::types::ReasoningEffort::Medium => "medium",
            crate::types::ReasoningEffort::High => "high",
            crate::types::ReasoningEffort::Xhigh => "xhigh",
            crate::types::ReasoningEffort::Max => "max",
            crate::types::ReasoningEffort::Ultra => "ultra",
        }),
        // Budget-style is Anthropic-native; OpenAI adapters drop it (no
        // equivalent on the wire).
        ReasoningConfig::Budget { .. } => None,
    }
}

/// Map an Anthropic [`ContentBlockParam`] image source to an OpenAI
/// `image_url` content part.
pub(crate) fn image_url_part(source: &ImageSource) -> Value {
    let url = match source {
        ImageSource::Base64 { media_type, data } => {
            format!("data:{media_type};base64,{data}")
        }
        ImageSource::Url { url } => url.clone(),
    };
    json!({"type": "image_url", "image_url": {"url": url}})
}

/// Translate a canonical [`MessageParam`] into one or more OpenAI message
/// objects (returned as `serde_json::Value`s).
///
/// A single Anthropic message can fan out because:
/// - `ToolResult` blocks become standalone `role:"tool"` messages on OpenAI.
/// - `ToolUse` blocks (assistant turn) become the `tool_calls` array on the
///   assistant message.
///
/// Callers prepend the `system` message separately (Chat) or fold it into
/// `instructions` (Responses).
pub(crate) fn translate_message(param: &MessageParam) -> Vec<Value> {
    let role = match param.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    match &param.content {
        MessageContent::Text(text) => vec![json!({"role": role, "content": text})],
        MessageContent::Blocks(blocks) => {
            let mut out: Vec<Value> = Vec::new();

            // Partition blocks: tool_results become standalone `tool` messages;
            // text/image blocks become content parts for the original role;
            // tool_use blocks become tool_calls on the assistant message.
            let mut content_parts: Vec<Value> = Vec::new();
            let mut tool_calls: Vec<Value> = Vec::new();

            for block in blocks {
                match block {
                    ContentBlockParam::Text { text } => {
                        content_parts.push(json!({"type": "text", "text": text}));
                    }
                    ContentBlockParam::Image { source } => {
                        content_parts.push(image_url_part(source));
                    }
                    ContentBlockParam::ToolUse { id, name, input } => {
                        // OpenAI wants `arguments` as a JSON **string**, not an
                        // object — it streams as text fragments.
                        let arguments =
                            serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
                        tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": arguments},
                        }));
                    }
                    ContentBlockParam::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        // Emit as a separate `tool` role message. OpenAI
                        // requires `tool_call_id` to match the originating
                        // tool call.
                        out.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id,
                            "content": content.clone().unwrap_or_default(),
                        }));
                    }
                    ContentBlockParam::Thinking { .. } => {
                        // Chat Completions has no reasoning-history channel;
                        // drop silently.
                    }
                }
            }

            // Emit the primary message (with content parts and/or tool_calls)
            // only if there's something to carry.
            if !content_parts.is_empty() || !tool_calls.is_empty() {
                let mut msg = serde_json::Map::new();
                msg.insert("role".to_string(), json!(role));
                if content_parts.len() == 1
                    && content_parts[0].get("type").and_then(|t| t.as_str()) == Some("text")
                {
                    // Single text-only message: collapse to a plain string for
                    // maximum provider compatibility (some OpenAI-compatible
                    // gateways reject array-form content for non-vision models).
                    msg.insert("content".to_string(), content_parts[0]["text"].clone());
                } else if !content_parts.is_empty() {
                    msg.insert("content".to_string(), Value::Array(content_parts));
                } else {
                    // tool_calls only — content must be null per OpenAI spec.
                    msg.insert("content".to_string(), Value::Null);
                }
                if !tool_calls.is_empty() {
                    msg.insert("tool_calls".to_string(), Value::Array(tool_calls));
                }
                out.push(Value::Object(msg));
            }

            out
        }
    }
}

/// Helper: parse a JSON body string, returning a typed [`AnthropicError`] on
/// failure.
pub(crate) fn parse_json(body: &str) -> Result<Value> {
    serde_json::from_str(body).map_err(|e| {
        AnthropicError::StreamError(format!(
            "failed to parse OpenAI response as JSON: {e}; body: {}",
            body.chars().take(500).collect::<String>()
        ))
    })
}
