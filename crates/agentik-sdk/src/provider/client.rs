use crate::Anthropic;
use crate::model::ModelInfo;
use crate::streaming::MessageStream;
use agentik_types::errors::AnthropicError;
use agentik_types::messages::{ContentBlock, Message, Role};
use agentik_types::messages::{ContentBlockParam, MessageContent, MessageCreateBuilder};
use agentik_types::tools::ToolDefinition;
use async_trait::async_trait;
use mockall::automock;

#[automock]
#[async_trait]
pub trait ApiClient: Send + Sync {
    async fn request(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model_info: &ModelInfo,
    ) -> Result<Message, AnthropicError>;

    async fn request_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model_info: &ModelInfo,
    ) -> Result<MessageStream, AnthropicError>;

    /// Like `request_stream` but also sets the top-level `system` field.
    /// Default implementation delegates to `request_stream` (ignoring
    /// `system`); the concrete `AnthropicApiClient` overrides to pass
    /// it through to `MessageCreateParams.system`.
    async fn request_stream_with_system(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model_info: &ModelInfo,
        system: Option<String>,
    ) -> Result<MessageStream, AnthropicError> {
        let _ = system;
        self.request_stream(messages, tools, model_info).await
    }

    async fn test_connection(&self) -> Result<(), AnthropicError>;
}

pub struct AnthropicApiClient {
    client: Anthropic,
}

impl AnthropicApiClient {
    pub fn new(client: Anthropic) -> Self {
        Self { client }
    }
}

fn content_block_to_param(block: ContentBlock) -> ContentBlockParam {
    match block {
        ContentBlock::Text { text } => ContentBlockParam::Text { text },
        ContentBlock::Thinking {
            thinking,
            signature,
        } => ContentBlockParam::Thinking {
            thinking,
            signature,
        },
        ContentBlock::Image { source } => ContentBlockParam::Image { source },
        ContentBlock::ToolUse { id, name, input } => {
            // Ensure input is a valid JSON object (dictionary). Some providers
            // (e.g. GLM) may emit tool_use blocks where the streaming JSON
            // accumulation failed to parse, leaving a String/Null instead of
            // an object. Replaying such blocks to another model triggers
            // "Input should be a valid dictionary" API errors.
            let input = if input.is_object() {
                input
            } else {
                serde_json::Value::Object(serde_json::Map::new())
            };
            ContentBlockParam::ToolUse { id, name, input }
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => ContentBlockParam::ToolResult {
            tool_use_id,
            content,
            is_error,
        },
    }
}

fn message_to_content(msg: Message, preserve_thinking: bool) -> MessageContent {
    MessageContent::Blocks(
        msg.content
            .into_iter()
            // Drop thinking blocks with empty signatures for models that
            // don't support thinking at all — some providers choke on the
            // thinking block type, returning empty responses or errors.
            //
            // For models that DO support thinking, preserve all thinking
            // blocks unconditionally. Even when thinking is "disabled" in
            // config, providers like DeepSeek and GLM default to thinking-on
            // and require prior thinking blocks to be passed back — stripping
            // them causes "content[].thinking must be passed back" API errors.
            .filter(|block| {
                preserve_thinking
                    || !matches!(
                        block,
                        ContentBlock::Thinking {
                            signature,
                            ..
                        } if signature.is_empty()
                    )
            })
            .map(content_block_to_param)
            .collect(),
    )
}

#[async_trait]
impl ApiClient for AnthropicApiClient {
    async fn request(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model_info: &ModelInfo,
    ) -> Result<Message, AnthropicError> {
        let max_tokens: u32 = model_info
            .max_output_tokens
            .max(1)
            .try_into()
            .unwrap_or(u32::MAX);
        let mut builder = MessageCreateBuilder::new(model_info.model_name.clone(), max_tokens);

        // Preserve thinking blocks for any model that supports thinking.
        // Even when thinking is "disabled" in config, providers like DeepSeek
        // and GLM may default to thinking-on and produce thinking blocks. If
        // those blocks are stripped on replay, the API rejects the request
        // with "content[].thinking must be passed back" errors.
        let preserve_thinking = model_info.supports_thinking;
        for msg in &messages {
            let content = message_to_content(msg.clone(), preserve_thinking);
            builder = match msg.role {
                Role::User => builder.message(Role::User, content),
                Role::Assistant => builder.message(Role::Assistant, content),
            };
        }

        builder = builder.tools(tools);
        builder = inject_thinking(builder, model_info);

        let params = builder.build();
        self.client.messages().create(params).await
    }

    async fn request_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model_info: &ModelInfo,
    ) -> Result<MessageStream, AnthropicError> {
        let max_tokens: u32 = model_info
            .max_output_tokens
            .max(1)
            .try_into()
            .unwrap_or(u32::MAX);
        let mut builder = MessageCreateBuilder::new(model_info.model_name.clone(), max_tokens);

        let preserve_thinking = model_info.supports_thinking;
        for msg in &messages {
            let content = message_to_content(msg.clone(), preserve_thinking);
            builder = match msg.role {
                Role::User => builder.message(Role::User, content),
                Role::Assistant => builder.message(Role::Assistant, content),
            };
        }

        builder = builder.tools(tools);
        builder = inject_thinking(builder, model_info);

        let params = builder.build();
        self.client.messages().create_stream(params).await
    }

    async fn request_stream_with_system(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model_info: &ModelInfo,
        system: Option<String>,
    ) -> Result<MessageStream, AnthropicError> {
        let max_tokens: u32 = model_info
            .max_output_tokens
            .max(1)
            .try_into()
            .unwrap_or(u32::MAX);
        let mut builder = MessageCreateBuilder::new(model_info.model_name.clone(), max_tokens);

        if let Some(s) = system {
            builder = builder.system(s);
        }

        let preserve_thinking = model_info.supports_thinking;
        for msg in &messages {
            let content = message_to_content(msg.clone(), preserve_thinking);
            builder = match msg.role {
                Role::User => builder.message(Role::User, content),
                Role::Assistant => builder.message(Role::Assistant, content),
            };
        }

        builder = builder.tools(tools);
        builder = inject_thinking(builder, model_info);

        let params = builder.build();
        self.client.messages().create_stream(params).await
    }

    async fn test_connection(&self) -> Result<(), AnthropicError> {
        self.client.test_connection().await
    }
}

/// Translate the model's thinking configuration into the appropriate
/// request-builder calls.
///
/// Three cases:
/// 1. `supports_thinking && thinking_enabled` → inject `thinking: { enabled,
///    budget }` + `reasoning_effort: max`.
/// 2. `supports_thinking && !thinking_enabled && !thinking_required` → inject
///    `thinking: { disabled }`.
///    Some providers (MiMo, DeepSeek, GLM) **default to thinking ON** when the
///    field is absent, which wastes tokens and produces empty-signature thinking
///    blocks that get stripped on replay — causing context discontinuity and
///    unreliable tool calling. Explicitly disabling avoids this.
/// 3. `thinking_required && !thinking_enabled` → keep thinking enabled and use
///    the lightest valid effort. GLM-5.3 rejects disabled thinking.
/// 4. `!supports_thinking` → no thinking field at all (the provider may not
///    recognise it).
fn inject_thinking(builder: MessageCreateBuilder, model_info: &ModelInfo) -> MessageCreateBuilder {
    if !model_info.supports_thinking {
        return builder;
    }
    let thinking_enabled = model_info.thinking_enabled || model_info.thinking_required;
    if !thinking_enabled {
        // Explicitly disable thinking so providers that default to thinking-ON
        // (e.g. MiMo) don't enter thinking mode unexpectedly.
        return builder.thinking(agentik_types::ThinkingConfig::disabled());
    }
    // Set both forms: Anthropic wires read `thinking`, OpenAI wires read
    // `reasoning`. Each wire picks the one it understands and ignores the
    // other. Use the strongest effort level available unless a required-thinking
    // model was explicitly configured as disabled, in which case low is the
    // closest supported lightweight mode.
    let effort = if model_info.thinking_required && !model_info.thinking_enabled {
        agentik_types::ReasoningEffort::Low
    } else {
        agentik_types::ReasoningEffort::Max
    };
    // GLM-5.3 accepts the enabled thinking type but does not document a token
    // budget. Preserve output capacity and send only the supported fields.
    let thinking = if model_info.thinking_required {
        agentik_types::ThinkingConfig {
            kind: agentik_types::ThinkingKind::Enabled,
            budget_tokens: None,
        }
    } else {
        let budget = model_info.thinking_budget.unwrap_or_else(|| {
            let half = (model_info.max_output_tokens / 2).max(1024) as u32;
            // Ensure budget stays below max_tokens to satisfy Anthropic's API
            // constraint (`budget_tokens` must be less than `max_tokens`).
            let max_tokens = model_info.max_output_tokens.max(1) as u32;
            half.min(max_tokens.saturating_sub(1)).max(1024)
        });
        agentik_types::ThinkingConfig::enabled(budget)
    };
    builder.thinking(thinking).reasoning_effort(effort)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::zai::{MODEL_GLM_5_3, ZaiProvider};
    use agentik_types::ReasoningConfig;

    fn glm_5_3_model() -> ModelInfo {
        ZaiProvider::preset_models()
            .into_iter()
            .find(|model| model.model_name == MODEL_GLM_5_3)
            .expect("GLM-5.3 preset exists")
    }

    #[test]
    fn glm_5_3_defaults_to_max_reasoning_effort() {
        let model = glm_5_3_model();
        let params = inject_thinking(
            MessageCreateBuilder::new(model.model_name.clone(), 1024),
            &model,
        )
        .build();

        assert!(
            params
                .thinking
                .as_ref()
                .is_some_and(|thinking| thinking.kind == agentik_types::ThinkingKind::Enabled)
        );
        assert!(
            params
                .thinking
                .as_ref()
                .is_some_and(|thinking| thinking.budget_tokens.is_none())
        );
        assert!(matches!(
            params.reasoning,
            Some(ReasoningConfig::Effort(agentik_types::ReasoningEffort::Max))
        ));
    }

    #[test]
    fn glm_5_3_disable_attempt_becomes_low_reasoning() {
        let mut model = glm_5_3_model();
        model.thinking_enabled = false;
        let params = inject_thinking(
            MessageCreateBuilder::new(model.model_name.clone(), 1024),
            &model,
        )
        .build();

        assert!(
            params
                .thinking
                .as_ref()
                .is_some_and(|thinking| thinking.kind == agentik_types::ThinkingKind::Enabled)
        );
        assert!(
            params
                .thinking
                .as_ref()
                .is_some_and(|thinking| thinking.budget_tokens.is_none())
        );
        assert!(matches!(
            params.reasoning,
            Some(ReasoningConfig::Effort(agentik_types::ReasoningEffort::Low))
        ));
    }
}
