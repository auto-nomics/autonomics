//! MiniMax adaptive thinking on the Anthropic-compatible Messages API.
//! Contract: https://platform.minimax.io/docs/api-reference/text-anthropic-api
use crate::types::errors::{AnthropicError, Result};
use crate::types::messages::{Message, MessageCreateParams};
use crate::types::reasoning::{ReasoningConfig, ReasoningEffort, ThinkingKind};
use crate::types::shared::RequestId;
use crate::types::streaming::MessageStreamEvent;
use crate::wire::anthropic::AnthropicWire;
use crate::wire::{ProtocolFeatures, StreamState, WireProtocol, WireRequest};
use serde_json::{Value, json};
#[derive(Debug, Clone, Copy, Default)]
pub struct MinimaxAnthropicWire;
fn failure(message: &str) -> AnthropicError {
    AnthropicError::Configuration {
        message: message.into(),
    }
}
impl WireProtocol for MinimaxAnthropicWire {
    fn id(&self) -> &'static str {
        "minimax_anthropic"
    }
    fn features(&self) -> ProtocolFeatures {
        AnthropicWire.features()
    }
    fn encode_request(&self, params: &MessageCreateParams, streaming: bool) -> Result<WireRequest> {
        let mut request = AnthropicWire.encode_request(params, streaming)?;
        let model = params.model.to_ascii_lowercase();
        let flash = model == "minimax-m3.1-flash" || model == "minimax-m3.1-flash-preview";
        let m3 = model == "minimax-m3";
        if !flash && !m3 {
            return Ok(request);
        }
        let disabled = params
            .thinking
            .as_ref()
            .is_some_and(|t| t.kind == ThinkingKind::Disabled);
        if flash && disabled {
            return Err(failure(
                "MiniMax M3.1 Flash requires adaptive thinking; disabled thinking is unsupported. Choose low or another supported effort.",
            ));
        }
        let mut body: Value = serde_json::from_slice(&request.body)
            .map_err(|_| failure("invalid MiniMax request body"))?;
        body.as_object_mut().unwrap().remove("reasoning");
        if flash || params.thinking.is_some() {
            body["thinking"] = if disabled {
                json!({"type":"disabled"})
            } else {
                json!({"type":"adaptive"})
            };
        }
        if flash && let Some(reasoning) = &params.reasoning {
            let effort = match reasoning {
                ReasoningConfig::Effort(ReasoningEffort::None | ReasoningEffort::Minimal) => {
                    return Err(failure(
                        "MiniMax M3.1 Flash supports low, medium, high, xhigh or max effort; none/minimal are unsupported.",
                    ));
                }
                ReasoningConfig::Effort(ReasoningEffort::Ultra) => "max",
                ReasoningConfig::Effort(effort) => effort.as_str(),
                ReasoningConfig::Budget { .. } => {
                    return Err(failure(
                        "MiniMax M3.1 Flash uses adaptive effort, not numeric reasoning budgets.",
                    ));
                }
            };
            body["output_config"] = json!({"effort":effort});
        }
        request.body =
            serde_json::to_vec(&body).map_err(|_| failure("cannot encode MiniMax request"))?;
        Ok(request)
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
    use crate::types::reasoning::ThinkingConfig;
    fn body(model: &str, effort: ReasoningEffort) -> Value {
        let params = MessageCreateBuilder::new(model, 32768)
            .user("hello")
            .thinking(ThinkingConfig::enabled(2048))
            .reasoning_effort(effort)
            .build();
        serde_json::from_slice(
            &MinimaxAnthropicWire
                .encode_request(&params, true)
                .unwrap()
                .body,
        )
        .unwrap()
    }
    #[test]
    fn current_flash_aliases_emit_adaptive_and_protocol_specific_effort() {
        for model in ["MiniMax-M3.1-Flash", "minimax-m3.1-flash-preview"] {
            for effort in [
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::Xhigh,
                ReasoningEffort::Max,
            ] {
                let value = body(model, effort);
                assert_eq!(value["thinking"], json!({"type":"adaptive"}));
                assert_eq!(value["output_config"]["effort"], effort.as_str());
                assert!(value.get("reasoning").is_none());
                assert!(value["thinking"].get("budget_tokens").is_none());
                assert_eq!(value["stream"], true);
                assert_eq!(value["max_tokens"], 32768);
            }
        }
        assert_eq!(
            body("MiniMax-M3.1-Flash", ReasoningEffort::Ultra)["output_config"]["effort"],
            "max"
        );
    }
    #[test]
    fn required_flash_rejects_disabled_or_unsupported_numeric_controls() {
        for effort in [ReasoningEffort::None, ReasoningEffort::Minimal] {
            let params = MessageCreateBuilder::new("MiniMax-M3.1-Flash", 32768)
                .reasoning_effort(effort)
                .build();
            assert!(MinimaxAnthropicWire.encode_request(&params, false).is_err());
        }
        let params = MessageCreateBuilder::new("MiniMax-M3.1-Flash", 32768)
            .thinking(ThinkingConfig::disabled())
            .build();
        assert!(MinimaxAnthropicWire.encode_request(&params, false).is_err());
        let params = MessageCreateBuilder::new("MiniMax-M3.1-Flash", 32768)
            .reasoning(ReasoningConfig::from_budget(2048))
            .build();
        assert!(MinimaxAnthropicWire.encode_request(&params, false).is_err());
    }
    #[test]
    fn optional_m3_and_legacy_messages_preserve_their_distinct_controls() {
        let value = body("MiniMax-M3", ReasoningEffort::Low);
        assert_eq!(value["thinking"], json!({"type":"adaptive"}));
        assert!(value.get("output_config").is_none());
        let params = MessageCreateBuilder::new("MiniMax-M3", 4096)
            .thinking(ThinkingConfig::disabled())
            .build();
        let value: Value = serde_json::from_slice(
            &MinimaxAnthropicWire
                .encode_request(&params, false)
                .unwrap()
                .body,
        )
        .unwrap();
        assert_eq!(value["thinking"], json!({"type":"disabled"}));
        let params = MessageCreateBuilder::new("MiniMax-M2.7", 4096)
            .thinking(ThinkingConfig::enabled(2048))
            .build();
        assert_eq!(
            MinimaxAnthropicWire
                .encode_request(&params, false)
                .unwrap()
                .body,
            AnthropicWire.encode_request(&params, false).unwrap().body
        );
    }
    #[test]
    fn registered_provider_builds_the_adaptive_adapter() {
        let kind = crate::provider::registry::wire_protocol(&crate::model::ProviderType::Minimax);
        let wire = crate::wire::build_wire(kind).unwrap();
        assert_eq!(wire.id(), "minimax_anthropic");
        let params = MessageCreateBuilder::new("MiniMax-M3", 4096)
            .user("hello")
            .build();
        let value: Value =
            serde_json::from_slice(&wire.encode_request(&params, false).unwrap().body).unwrap();
        assert!(value.get("thinking").is_none());
    }
}
