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
use crate::wire::{ProtocolFeatures, ThinkingSupport, WireProtocol, WireRequest};

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

    fn encode_request(
        &self,
        params: &MessageCreateParams,
        streaming: bool,
    ) -> Result<WireRequest> {
        // When the caller asked for streaming, ensure the body advertises it.
        // We avoid mutating the caller's `params` (it is borrowed) and instead
        // patch a serialised copy — serde_json round-trip is cheap relative to
        // the network round-trip that follows.
        let body_bytes = if streaming && params.stream != Some(true) {
            let mut value = serde_json::to_value(params).map_err(|e| AnthropicError::Connection {
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
            return Err(AnthropicError::from_status(
                status,
                body.to_string(),
            ));
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
}
