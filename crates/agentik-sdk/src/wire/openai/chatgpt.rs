//! ChatGPT 订阅后端的 Codex Responses wire 协议
//! （`POST /backend-api/codex/responses`）。
//!
//! ChatGPT 后端说的是 OpenAI Responses 协议（请求/SSE 事件形状与
//! `POST /v1/responses` 相同），差异仅在端点路径和要求的额外头。因此
//! 本 wire 全量委托 [`OpenAiResponsesWire`]，只覆写端点并附加静态头；
//! 动态头（`chatgpt-account-id`、Bearer token）由 HTTP 认证层
//! （[`AuthMethod::Chatgpt`](crate::http::auth::AuthMethod::Chatgpt)）注入。

use crate::types::errors::Result;
use crate::types::messages::{Message, MessageCreateParams};
use crate::types::shared::RequestId;
use crate::types::streaming::MessageStreamEvent;
use crate::wire::openai::responses::OpenAiResponsesWire;
use crate::wire::{ProtocolFeatures, StreamState, WireProtocol, WireRequest};

/// ChatGPT 后端的 Codex Responses 端点路径。
pub const ENDPOINT_PATH: &str = "/backend-api/codex/responses";

/// ⚠️ 后端校验的客户端来源标识，当前取 Codex CLI 的值（第三方复用
/// 的通行做法，如 Roo Code）。若官方收紧需实测修正——集中在此常量。
pub const ORIGINATOR: &str = "codex_cli_rs";

/// Responses 早期接入的 beta 标记，ChatGPT 后端仍要求携带。
const OPENAI_BETA: &str = "responses=experimental";

/// Wire-protocol impl for the ChatGPT backend (Codex Responses endpoint).
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatgptResponsesWire;

impl ChatgptResponsesWire {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl WireProtocol for ChatgptResponsesWire {
    fn id(&self) -> &'static str {
        "chatgpt_responses"
    }

    fn features(&self) -> ProtocolFeatures {
        OpenAiResponsesWire.features()
    }

    fn encode_request(&self, params: &MessageCreateParams, streaming: bool) -> Result<WireRequest> {
        let mut req = OpenAiResponsesWire.encode_request(params, streaming)?;
        req.endpoint_path = ENDPOINT_PATH.to_string();
        req.headers.push(("OpenAI-Beta", OPENAI_BETA));
        req.headers.push(("originator", ORIGINATOR));
        // ChatGPT 后端不接受 max_output_tokens（Codex CLI 从不下发；
        // 实测 400 "Unsupported parameter: max_output_tokens"）。上游
        // 参数层恒定携带（max(1) 兜底），在此剥除，让服务器用默认上限。
        if let Ok(mut body) = serde_json::from_slice::<serde_json::Value>(&req.body)
            && body
                .as_object_mut()
                .is_some_and(|o| o.remove("max_output_tokens").is_some())
            && let Ok(bytes) = serde_json::to_vec(&body)
        {
            req.body = bytes;
        }
        Ok(req)
    }

    fn decode_response(
        &self,
        status: u16,
        body: &str,
        request_id: Option<RequestId>,
    ) -> Result<Message> {
        OpenAiResponsesWire.decode_response(status, body, request_id)
    }

    fn adapt_sse_event(
        &self,
        event_type: &str,
        data: &str,
        state: &mut StreamState,
    ) -> Result<Option<MessageStreamEvent>> {
        OpenAiResponsesWire.adapt_sse_event(event_type, data, state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::messages::MessageCreateBuilder;

    #[test]
    fn overrides_endpoint_and_appends_static_headers() {
        let wire = ChatgptResponsesWire;
        let params = MessageCreateBuilder::new("gpt-5.5-codex", 1024)
            .user("hi")
            .build();
        let req = wire.encode_request(&params, false).unwrap();
        assert_eq!(req.endpoint_path, "/backend-api/codex/responses");
        assert!(
            req.headers
                .iter()
                .any(|(k, v)| (*k, *v) == ("OpenAI-Beta", "responses=experimental"))
        );
        assert!(
            req.headers
                .iter()
                .any(|(k, v)| (*k, *v) == ("originator", ORIGINATOR))
        );
    }

    #[test]
    fn build_wire_resolves_chatgpt_kind() {
        let w = crate::wire::build_wire(crate::wire::WireProtocolKind::ChatgptResponses).unwrap();
        assert_eq!(w.id(), "chatgpt_responses");
    }

    #[test]
    fn strips_max_output_tokens_for_chatgpt_backend() {
        let wire = ChatgptResponsesWire;
        let params = MessageCreateBuilder::new("gpt-6-astra", 128_000)
            .user("hi")
            .build();
        let req = wire.encode_request(&params, false).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert!(body.get("max_output_tokens").is_none(), "body: {body}");
        assert_eq!(body["input"][0]["content"][0]["text"], "hi");
    }
}
