//! Zhipu / BigModel Anthropic-compatible wire protocol.
//!
//! BigModel accepts the Anthropic Messages shape but uses the GLM parameter
//! name `reasoning_effort` rather than the SDK's neutral `reasoning` field.

use crate::types::errors::{AnthropicError, Result};
use crate::types::messages::{Message, MessageCreateParams};
use crate::types::reasoning::{ReasoningConfig, ReasoningEffort, ThinkingConfig, ThinkingKind};
use crate::types::shared::RequestId;
use crate::types::streaming::MessageStreamEvent;
use crate::wire::anthropic::AnthropicWire;
use crate::wire::{ProtocolFeatures, StreamState, ThinkingSupport, WireProtocol, WireRequest};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Default)]
pub struct ZaiAnthropicWire;

impl ZaiAnthropicWire {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

fn is_glm_5_3(model: &str) -> bool {
    model == "glm-5.3" || model.starts_with("glm-5.3[")
}

fn glm_reasoning_effort(effort: ReasoningEffort) -> ReasoningEffort {
    match effort {
        ReasoningEffort::None | ReasoningEffort::Minimal | ReasoningEffort::Low => {
            ReasoningEffort::Low
        }
        ReasoningEffort::Medium | ReasoningEffort::High => ReasoningEffort::High,
        ReasoningEffort::Xhigh | ReasoningEffort::Max | ReasoningEffort::Ultra => {
            ReasoningEffort::Max
        }
    }
}

impl WireProtocol for ZaiAnthropicWire {
    fn id(&self) -> &'static str {
        "zai_anthropic"
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
        let request = AnthropicWire.encode_request(params, streaming)?;
        let mut body: Value =
            serde_json::from_slice(&request.body).map_err(|e| AnthropicError::Connection {
                message: format!("failed to parse Anthropic request body: {e}"),
            })?;
        let object = body
            .as_object_mut()
            .ok_or_else(|| AnthropicError::Connection {
                message: "Anthropic request body must be a JSON object".to_string(),
            })?;

        let mut forced_low = false;
        if is_glm_5_3(&params.model) {
            let thinking_disabled = params
                .thinking
                .as_ref()
                .is_some_and(|thinking| thinking.kind == ThinkingKind::Disabled);
            if thinking_disabled {
                object.insert(
                    "thinking".to_string(),
                    serde_json::to_value(ThinkingConfig {
                        kind: ThinkingKind::Enabled,
                        budget_tokens: None,
                    })
                    .map_err(|e| AnthropicError::Connection {
                        message: format!("failed to serialise thinking config: {e}"),
                    })?,
                );
                forced_low = true;
            }
        }

        if let Some(reasoning) = &params.reasoning {
            object.remove("reasoning");
            match reasoning {
                ReasoningConfig::Effort(effort) => {
                    object.insert(
                        "reasoning_effort".to_string(),
                        json!(glm_reasoning_effort(*effort)),
                    );
                }
                ReasoningConfig::Budget { budget_tokens } => {
                    if params.thinking.is_none() {
                        object.insert(
                            "thinking".to_string(),
                            json!({"type": "enabled", "budget_tokens": budget_tokens}),
                        );
                    }
                }
            }
        } else if forced_low {
            object.insert("reasoning_effort".to_string(), json!("low"));
        }

        Ok(WireRequest {
            endpoint_path: request.endpoint_path,
            body: serde_json::to_vec(&body).map_err(|e| AnthropicError::Connection {
                message: format!("failed to serialise Z.ai request body: {e}"),
            })?,
            headers: request.headers,
        })
    }

    fn decode_response(
        &self,
        status: u16,
        body: &str,
        request_id: Option<RequestId>,
    ) -> Result<Message> {
        AnthropicWire.decode_response(status, body, request_id)
    }

    fn adapt_sse_event(
        &self,
        event_type: &str,
        data: &str,
        state: &mut StreamState,
    ) -> Result<Option<MessageStreamEvent>> {
        AnthropicWire.adapt_sse_event(event_type, data, state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::messages::MessageCreateBuilder;
    use crate::wire::anthropic::ANTHROPIC_VERSION;

    #[test]
    fn maps_reasoning_to_zhipu_effort_field() {
        let wire = ZaiAnthropicWire;
        let params = MessageCreateBuilder::new("glm-5.3", 1024)
            .user("hello")
            .reasoning_effort(ReasoningEffort::Medium)
            .build();
        let request = wire.encode_request(&params, false).unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["reasoning_effort"], "high");
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn glm_5_3_never_sends_disabled_thinking() {
        let wire = ZaiAnthropicWire;
        let params = MessageCreateBuilder::new("glm-5.3[1m]", 1024)
            .user("hello")
            .thinking(ThinkingConfig::disabled())
            .build();
        let request = wire.encode_request(&params, false).unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "low");
        assert!(body["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn other_glm_models_may_disable_thinking() {
        let wire = ZaiAnthropicWire;
        let params = MessageCreateBuilder::new("glm-5.2", 1024)
            .user("hello")
            .thinking(ThinkingConfig::disabled())
            .build();
        let request = wire.encode_request(&params, false).unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["thinking"]["type"], "disabled");
    }

    #[test]
    fn uses_anthropic_version_header() {
        let wire = ZaiAnthropicWire;
        let params = MessageCreateBuilder::new("glm-5.3", 1024)
            .user("hello")
            .build();
        let request = wire.encode_request(&params, false).unwrap();
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| *name == "anthropic-version" && *value == ANTHROPIC_VERSION)
        );
    }
}
