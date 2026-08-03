//! Wire-protocol abstraction for message requests.
//!
//! The SDK's public surface (`MessageCreateParams`, `Message`, `MessageStream`)
//! is shaped after the Anthropic Messages API. Each concrete provider, however,
//! may speak a different wire protocol: Anthropic Messages, OpenAI Chat
//! Completions, or the OpenAI Responses API.
//!
//! [`WireProtocol`] is the seam between the protocol-neutral SDK front-end and
//! a concrete wire format. The SDK holds one `Arc<dyn WireProtocol>` and routes
//! every request through it, so the front-end stays uniform while the protocol
//! impl is freely swappable.
//!
//! Today only [`AnthropicWire`] is implemented; OpenAI Chat and Responses
//! adapters ship in subsequent milestones. The trait surface below is shaped to
//! accommodate them without further API churn.
//!
//! ## Canonical IR
//!
//! [`MessageCreateParams`](crate::types::messages::MessageCreateParams) — already
//! in Anthropic shape — serves as the SDK's internal intermediate
//! representation. Each protocol impl is a bidirectional translator between
//! this IR and the on-the-wire JSON.

pub mod anthropic;
pub mod openai;

pub use anthropic::AnthropicWire;
pub use openai::{OpenAiChatWire, OpenAiResponsesWire};

use crate::model::ProviderType;
use crate::types::errors::Result;
use crate::types::messages::{Message, MessageCreateParams};
use crate::types::shared::RequestId;
use crate::types::streaming::MessageStreamEvent;

/// Identifies a wire protocol on a [`ProviderType`] / `ProviderConfig`.
///
/// Stored alongside a provider's connection metadata so the SDK can build the
/// matching [`WireProtocol`] impl via [`build_wire`]. All existing presets
/// default to [`WireProtocolKind::Anthropic`] since they expose
/// Anthropic-compatible gateways.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireProtocolKind {
    /// Anthropic Messages API (`POST /v1/messages`).
    #[default]
    Anthropic,
    /// OpenAI Chat Completions API (`POST /v1/chat/completions`).
    ///
    /// Implementation lands in a later milestone; [`build_wire`] currently
    /// returns an error for this variant.
    OpenaiChat,
    /// OpenAI Responses API (`POST /v1/responses`).
    ///
    /// Implementation lands in a later milestone; [`build_wire`] currently
    /// returns an error for this variant.
    OpenaiResponses,
}

impl WireProtocolKind {
    /// Lowercase wire identifier used in logs, config dumps, and tests.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenaiChat => "openai_chat",
            Self::OpenaiResponses => "openai_responses",
        }
    }
}

impl std::fmt::Display for WireProtocolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which reasoning configuration style a [`WireProtocol`] understands.
///
/// The SDK accepts both forms on [`MessageCreateParams`](crate::types::messages::MessageCreateParams)
/// (see `agentik_types::reasoning`); each protocol impl advertises which form
/// it can carry natively and translates the other as best it can.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThinkingSupport {
    /// Reasoning configuration is not supported; any value is silently dropped.
    #[default]
    Off,
    /// Anthropic-style `thinking: { type: "enabled", budget_tokens: N }`.
    AnthropicBudget,
    /// OpenAI-style `reasoning: { effort: "low" | "medium" | "high" }`.
    OpenaiEffort,
}

/// Protocol-level capability upper bounds advertised by a [`WireProtocol`].
///
/// The front-end consults these to decide whether to surface a feature (e.g.
/// signed thinking) to callers or to degrade gracefully.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProtocolFeatures {
    /// How (if at all) reasoning intensity is conveyed on the wire.
    pub thinking: ThinkingSupport,
    /// Whether tool-use blocks survive a round-trip on this protocol.
    pub tool_use: bool,
    /// Whether parallel tool calls are a first-class request field.
    pub parallel_tool_calls: bool,
    /// Whether image inputs can be encoded.
    pub image_input: bool,
    /// Whether the system prompt is carried as a top-level field (Anthropic)
    /// versus folded into the message list as a `system` role (OpenAI).
    pub system_as_message: bool,
    /// Whether thinking blocks carry a verifiable signature on return.
    ///
    /// Anthropic signs thinking blocks so they can be replayed in later turns;
    /// the OpenAI Responses API only exposes unsigned reasoning summaries.
    pub signed_thinking: bool,
}

/// Endpoint metadata emitted by [`WireProtocol::encode_request`].
#[derive(Debug, Clone)]
pub struct WireRequest {
    /// Path appended to the configured base URL (e.g. `"/v1/messages"`).
    pub endpoint_path: String,
    /// Serialised request body bytes.
    pub body: Vec<u8>,
    /// Extra static headers (auth/Content-Type are added by the HTTP layer).
    pub headers: Vec<(&'static str, &'static str)>,
}

/// Mutable per-stream state used by [`WireProtocol::adapt_sse_event`].
///
/// Anthropic SSE events are self-describing (each carries its own block index),
/// so [`AnthropicWire`] barely touches this struct. The OpenAI adapters, by
/// contrast, drive a state machine over it: OpenAI's chunk stream doesn't
/// annotate block starts/stops, so the adapter must synthesise the
/// Anthropic-shaped `ContentBlockStart`/`ContentBlockDelta`/`ContentBlockStop`
/// events from the raw delta stream and track which Anthropic-side block index
/// corresponds to which OpenAI tool-call index.
#[derive(Debug, Default)]
pub struct StreamState {
    /// The model name seen on the first chunk, used to fill in the synthesised
    /// `MessageStart.message.model` field on protocols that don't send a
    /// top-level message header (OpenAI Chat / Responses).
    pub model: Option<String>,
    /// The response id seen on the first chunk, used for `MessageStart.message.id`.
    pub response_id: Option<String>,
    /// Whether the `MessageStart` event has already been emitted.
    pub started: bool,
    /// Whether the text content block (Anthropic index 0) has been opened.
    pub text_block_open: bool,
    /// Mapping from OpenAI tool-call index → Anthropic block index + accumulator.
    /// OpenAI streams tool-call arguments as incremental string fragments that
    /// must be re-emitted as `ContentBlockDelta::InputJsonDelta`.
    pub tool_calls: std::collections::BTreeMap<u32, ToolCallSlot>,
    /// The next Anthropic-side block index to assign (text=0, tools=1..).
    pub next_block_index: usize,
    /// Accumulated stop reason, emitted in the final `MessageDelta`.
    pub stop_reason: Option<crate::types::StopReason>,
    /// Accumulated usage, emitted in the final `MessageDelta`.
    pub usage: Option<crate::types::Usage>,
    /// Blocks accumulated by the adapter so the synthesised `MessageStop`
    /// / `MessageStart` can carry a consistent snapshot.
    pub blocks: Vec<crate::types::ContentBlock>,
}

/// Per-tool-call slot tracked while streaming OpenAI tool-call deltas.
#[derive(Debug, Default)]
pub struct ToolCallSlot {
    /// Anthropic-side content-block index assigned to this tool call.
    pub block_index: usize,
    /// OpenAI-side tool-call id (set on the first delta that carries it).
    pub id: String,
    /// Function name (set on the first delta that carries it).
    pub name: String,
    /// Accumulated argument fragments (the raw JSON string so far).
    pub arguments: String,
    /// Whether a `ContentBlockStart` has been emitted for this slot.
    pub block_started: bool,
    /// Whether a `ContentBlockStop` has been emitted for this slot.
    pub block_stopped: bool,
}

/// Trait abstracting a wire protocol for message requests.
///
/// All encode/decode methods are synchronous: they only transform bytes, never
/// perform I/O. The HTTP send/retry loop lives in the resource layer.
pub trait WireProtocol: Send + Sync {
    /// Wire identifier (matches [`WireProtocolKind::as_str`] for built-ins).
    fn id(&self) -> &'static str;

    /// Capability upper bounds for this protocol impl.
    fn features(&self) -> ProtocolFeatures {
        ProtocolFeatures::default()
    }

    /// Encode the canonical IR into a wire-specific HTTP request descriptor.
    ///
    /// `streaming` is true when the request targets the streaming endpoint; for
    /// most protocols this only flips a `stream: true` field inside the body.
    fn encode_request(
        &self,
        params: &MessageCreateParams,
        streaming: bool,
    ) -> Result<WireRequest>;

    /// Decode a non-streaming response body into the canonical [`Message`].
    ///
    /// `status` is the HTTP status code; impls may use it to attach richer
    /// error context. `request_id` is the value extracted from the
    /// `request-id` response header, if any.
    fn decode_response(
        &self,
        status: u16,
        body: &str,
        request_id: Option<RequestId>,
    ) -> Result<Message>;

    /// Translate one raw SSE event into zero or more canonical
    /// [`MessageStreamEvent`]s.
    ///
    /// `event_type` is the SSE `event:` field (empty for OpenAI Chat, which
    /// doesn't name its events); `data` is the raw `data:` payload (already
    /// stripped of the `data: ` prefix by the SSE parser). `state` carries
    /// per-stream accumulator state so protocols that lack self-describing
    /// events (OpenAI) can synthesise the Anthropic-shaped block start/stop
    /// sequence.
    ///
    /// Returning `Ok(None)` skips the event (e.g. Anthropic `ping`, OpenAI
    /// `[DONE]` sentinel — though the latter typically triggers `MessageStop`).
    fn adapt_sse_event(
        &self,
        event_type: &str,
        data: &str,
        state: &mut StreamState,
    ) -> Result<Option<MessageStreamEvent>>;
}

/// Build the [`WireProtocol`] impl matching a [`WireProtocolKind`].
///
/// # Errors
///
/// Currently always returns `Ok`; the match is exhaustive over the three
/// implemented variants. The `Result` is retained so future variants can
/// signal unsupported kinds without breaking the call site.
pub fn build_wire(kind: WireProtocolKind) -> Result<std::sync::Arc<dyn WireProtocol>> {
    match kind {
        WireProtocolKind::Anthropic => Ok(std::sync::Arc::new(AnthropicWire)),
        WireProtocolKind::OpenaiChat => Ok(std::sync::Arc::new(OpenAiChatWire)),
        WireProtocolKind::OpenaiResponses => Ok(std::sync::Arc::new(OpenAiResponsesWire)),
    }
}

/// Resolves the wire protocol kind advertised by a [`ProviderType`].
///
/// Thin wrapper over [`crate::provider::registry::wire_protocol`]; exposed from
/// the `wire` module so callers resolving a protocol impl don't need to import
/// the provider registry.
#[must_use]
pub fn wire_protocol_for_provider(provider_type: &ProviderType) -> WireProtocolKind {
    crate::provider::registry::wire_protocol(provider_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::messages::MessageCreateBuilder;
    use agentik_types::reasoning::{ReasoningConfig, ReasoningEffort, ThinkingConfig};

    #[test]
    fn anthropic_wire_encodes_canonical_body() {
        let wire = AnthropicWire;
        let params = MessageCreateBuilder::new("claude-sonnet-5", 1024)
            .user("hi")
            .build();
        let req = wire.encode_request(&params, false).unwrap();
        assert_eq!(req.endpoint_path, "/v1/messages");
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["model"], "claude-sonnet-5");
        assert_eq!(body["max_tokens"], 1024);
        // No `stream` field when streaming=false.
        assert!(body.get("stream").is_none());
    }

    #[test]
    fn anthropic_wire_sets_stream_flag_when_streaming() {
        let wire = AnthropicWire;
        let params = MessageCreateBuilder::new("claude-sonnet-5", 1024)
            .user("hi")
            .build();
        let req = wire.encode_request(&params, true).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn anthropic_wire_decodes_response() {
        let wire = AnthropicWire;
        let body = r#"{
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{"type":"text","text":"hi"}],
            "model": "claude-sonnet-5",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }"#;
        let msg = wire
            .decode_response(200, body, Some(RequestId::new("req-7")))
            .unwrap();
        assert_eq!(msg.id, "msg_1");
        assert_eq!(msg.role, crate::types::messages::Role::Assistant);
        assert_eq!(msg.request_id.expect("request id set").0, "req-7");
    }

    #[test]
    fn anthropic_wire_emits_thinking_block_in_body() {
        let wire = AnthropicWire;
        let params = MessageCreateBuilder::new("claude-sonnet-5", 4096)
            .user("solve it")
            .thinking(ThinkingConfig::enabled(8192))
            .build();
        let req = wire.encode_request(&params, false).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 8192);
    }

    #[test]
    fn anthropic_wire_advertises_budget_thinking_support() {
        let wire = AnthropicWire;
        let features = wire.features();
        assert_eq!(
            features.thinking,
            crate::wire::ThinkingSupport::AnthropicBudget
        );
        assert!(features.signed_thinking);
    }

    #[test]
    fn build_wire_returns_each_protocol() {
        let w = build_wire(WireProtocolKind::Anthropic).unwrap();
        assert_eq!(w.id(), "anthropic");
        let w = build_wire(WireProtocolKind::OpenaiChat).unwrap();
        assert_eq!(w.id(), "openai_chat");
        let w = build_wire(WireProtocolKind::OpenaiResponses).unwrap();
        assert_eq!(w.id(), "openai_responses");
    }

    #[test]
    fn params_carry_both_thinking_and_reasoning() {
        // The IR accepts both forms; the wire protocol picks one.
        let params = MessageCreateBuilder::new("claude-sonnet-5", 4096)
            .thinking(ThinkingConfig::enabled(4096))
            .reasoning(ReasoningConfig::from_effort(ReasoningEffort::High))
            .build();
        assert!(params.thinking.is_some());
        assert!(params.reasoning.is_some());
    }

    #[test]
    fn reasoning_effort_helper_sets_config() {
        let params = MessageCreateBuilder::new("claude-sonnet-5", 1024)
            .reasoning_effort(ReasoningEffort::Xhigh)
            .build();
        match params.reasoning.expect("reasoning set") {
            ReasoningConfig::Effort(e) => assert_eq!(e, ReasoningEffort::Xhigh),
            other => panic!("expected Effort, got {other:?}"),
        }
    }

    #[test]
    fn known_presets_default_to_anthropic_wire() {
        for kind in [
            WireProtocolKind::Anthropic,
            WireProtocolKind::OpenaiChat,
            WireProtocolKind::OpenaiResponses,
        ] {
            assert_eq!(kind.as_str(), match kind {
                WireProtocolKind::Anthropic => "anthropic",
                WireProtocolKind::OpenaiChat => "openai_chat",
                WireProtocolKind::OpenaiResponses => "openai_responses",
            });
        }
        let p = ProviderType::Mimo;
        assert_eq!(wire_protocol_for_provider(&p), WireProtocolKind::Anthropic);
    }

    #[test]
    fn all_builtin_provider_types_advertise_anthropic() {
        use crate::provider::registry::wire_protocol;
        for p in [
            ProviderType::Deepseek,
            ProviderType::Mimo,
            ProviderType::Minimax,
            ProviderType::Moonshot,
            ProviderType::Zai,
            ProviderType::Sensenova,
        ] {
            assert_eq!(wire_protocol(&p), WireProtocolKind::Anthropic, "{p}");
        }
        // Custom providers default to the Anthropic-compatible wire.
        assert_eq!(
            wire_protocol(&ProviderType::Custom("any-openai".into())),
            WireProtocolKind::Anthropic
        );
    }
}
