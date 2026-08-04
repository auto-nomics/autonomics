//! Anthropic Messages API wire protocol.
//!
//! This is the SDK's native wire format — the public types
//! ([`MessageCreateParams`](crate::types::messages::MessageCreateParams),
//! [`Message`](crate::types::messages::Message), `MessageStreamEvent`) already
//! mirror Anthropic's JSON shapes, so the encoder/decoder are essentially
//! `serde_json` identity passes with the right endpoint metadata bolted on.
//!
//! Behaviour previously hard-coded in `MessagesResource` (endpoint path,
//! `anthropic-version` header, body serialisation, response deserialisation)
//! now lives here so the resource layer is protocol-neutral.

use crate::types::errors::{AnthropicError, Result};
use crate::types::messages::{Message, MessageCreateParams};
use crate::types::shared::RequestId;
use crate::types::streaming::MessageStreamEvent;
use crate::types::{ContentBlock, ContentBlockDelta, MessageDelta, MessageDeltaUsage};
use crate::wire::{ProtocolFeatures, StreamState, ThinkingSupport, WireProtocol, WireRequest};

/// Wire-protocol impl for the Anthropic Messages API (`POST /v1/messages`).
#[derive(Debug, Clone, Copy, Default)]
pub struct AnthropicWire;

/// Anthropic API version sent on every request via the `anthropic-version`
/// header. (The auth handler already adds this for `x-api-key`/Bearer auth,
/// but emitting it here keeps the wire self-describing for callers that bypass
/// the auth handler.)
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Endpoint path appended to the configured base URL.
pub const ENDPOINT_PATH: &str = "/v1/messages";

impl AnthropicWire {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl WireProtocol for AnthropicWire {
    fn id(&self) -> &'static str {
        "anthropic"
    }

    fn features(&self) -> ProtocolFeatures {
        ProtocolFeatures {
            thinking: ThinkingSupport::AnthropicBudget,
            tool_use: true,
            parallel_tool_calls: true,
            image_input: true,
            system_as_message: false,
            signed_thinking: true,
        }
    }

    fn encode_request(&self, params: &MessageCreateParams, streaming: bool) -> Result<WireRequest> {
        // When the caller asked for streaming, ensure the body advertises it.
        // We avoid mutating the caller's `params` (it is borrowed) and instead
        // patch a serialised copy — serde_json round-trip is cheap relative to
        // the network round-trip that follows.
        let body_bytes = if streaming && params.stream != Some(true) {
            let mut value =
                serde_json::to_value(params).map_err(|e| AnthropicError::Connection {
                    message: format!("failed to serialise request body: {e}"),
                })?;
            if let Some(obj) = value.as_object_mut() {
                obj.insert("stream".to_string(), serde_json::Value::Bool(true));
            }
            serde_json::to_vec(&value).map_err(|e| AnthropicError::Connection {
                message: format!("failed to serialise request body: {e}"),
            })?
        } else {
            serde_json::to_vec(params).map_err(|e| AnthropicError::Connection {
                message: format!("failed to serialise request body: {e}"),
            })?
        };

        Ok(WireRequest {
            endpoint_path: ENDPOINT_PATH.to_string(),
            body: body_bytes,
            headers: vec![("anthropic-version", ANTHROPIC_VERSION)],
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
        let mut message: Message = serde_json::from_str(body).map_err(|e| {
            AnthropicError::from_status(
                status,
                format!(
                    "failed to parse response as JSON: {e}, body: {}",
                    body.chars().take(500).collect::<String>()
                ),
            )
        })?;
        message.request_id = request_id;
        Ok(message)
    }

    fn adapt_sse_event(
        &self,
        event_type: &str,
        data: &str,
        _state: &mut StreamState,
    ) -> Result<Option<MessageStreamEvent>> {
        // Anthropic SSE events are self-describing: the `event:` field names
        // the message type and the `data:` payload is the typed JSON body.
        // State on `StreamState` is unused for this protocol.
        match event_type {
            // Standard Anthropic SSE: full MessageStreamEvent in the payload.
            "message" | "" => serde_json::from_str::<MessageStreamEvent>(data)
                .map(Some)
                .map_err(|e| {
                    AnthropicError::StreamError(format!("Failed to parse SSE event: {e}"))
                }),

            "message_start" => {
                // The Anthropic spec ships `{ "type": "message_start", message: {…} }`,
                // but some Anthropic-compatible gateways ship the `Message`
                // body directly. Try both.
                serde_json::from_str::<Message>(data)
                    .map(|message| Some(MessageStreamEvent::MessageStart { message }))
                    .or_else(|_| {
                        let value: serde_json::Value = serde_json::from_str(data).map_err(|e| {
                            AnthropicError::StreamError(format!(
                                "Failed to parse message_start as JSON: {e}"
                            ))
                        })?;
                        let message_value = value.get("message").ok_or_else(|| {
                            AnthropicError::StreamError(
                                "message_start event missing message field".to_string(),
                            )
                        })?;
                        let message: Message = serde_json::from_value(message_value.clone())
                            .map_err(|e| {
                                AnthropicError::StreamError(format!(
                                    "Failed to parse nested message: {e}"
                                ))
                            })?;
                        Ok(Some(MessageStreamEvent::MessageStart { message }))
                    })
            }

            "content_block_start" => {
                let value: serde_json::Value = serde_json::from_str(data).map_err(|e| {
                    AnthropicError::StreamError(format!(
                        "Failed to parse content_block_start event: {e}"
                    ))
                })?;
                let index = value["index"].as_u64().unwrap_or(0) as usize;
                let content_block: ContentBlock =
                    serde_json::from_value(value["content_block"].clone()).map_err(|e| {
                        AnthropicError::StreamError(format!(
                            "Failed to parse content_block in content_block_start: {e}"
                        ))
                    })?;
                Ok(Some(MessageStreamEvent::ContentBlockStart {
                    content_block,
                    index,
                }))
            }

            "content_block_delta" => {
                let value: serde_json::Value = serde_json::from_str(data).map_err(|e| {
                    AnthropicError::StreamError(format!(
                        "Failed to parse content_block_delta event: {e}"
                    ))
                })?;
                let index = value["index"].as_u64().unwrap_or(0) as usize;
                let delta: ContentBlockDelta = serde_json::from_value(value["delta"].clone())
                    .map_err(|e| {
                        AnthropicError::StreamError(format!(
                            "Failed to parse delta in content_block_delta: {e}"
                        ))
                    })?;
                Ok(Some(MessageStreamEvent::ContentBlockDelta { delta, index }))
            }

            "content_block_stop" => {
                let value: serde_json::Value = serde_json::from_str(data).map_err(|e| {
                    AnthropicError::StreamError(format!(
                        "Failed to parse content_block_stop event: {e}"
                    ))
                })?;
                let index = value["index"].as_u64().unwrap_or(0) as usize;
                Ok(Some(MessageStreamEvent::ContentBlockStop { index }))
            }

            "message_delta" => {
                let value: serde_json::Value = serde_json::from_str(data).map_err(|e| {
                    AnthropicError::StreamError(format!("Failed to parse message_delta event: {e}"))
                })?;
                let delta: MessageDelta =
                    serde_json::from_value(value["delta"].clone()).map_err(|e| {
                        AnthropicError::StreamError(format!("Failed to parse delta: {e}"))
                    })?;
                let usage: MessageDeltaUsage = serde_json::from_value(value["usage"].clone())
                    .map_err(|e| {
                        AnthropicError::StreamError(format!("Failed to parse usage: {e}"))
                    })?;
                Ok(Some(MessageStreamEvent::MessageDelta { delta, usage }))
            }

            "message_stop" => Ok(Some(MessageStreamEvent::MessageStop)),

            // `ping` and any unknown event types are skipped silently.
            _ => Ok(None),
        }
    }
}
