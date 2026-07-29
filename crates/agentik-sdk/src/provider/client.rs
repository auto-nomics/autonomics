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
        ContentBlock::ToolUse { id, name, input } => ContentBlockParam::ToolUse { id, name, input },
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

fn message_to_content(msg: Message) -> MessageContent {
    MessageContent::Blocks(
        msg.content
            .into_iter()
            // Drop thinking blocks with empty signatures — some providers
            // (ZAI, MiniMax) don't support signed thinking and choke on
            // the empty value, returning empty responses or errors.
            .filter(|block| {
                !matches!(
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

        for msg in &messages {
            let content = message_to_content(msg.clone());
            builder = match msg.role {
                Role::User => builder.message(Role::User, content),
                Role::Assistant => builder.message(Role::Assistant, content),
            };
        }

        builder = builder.tools(tools);

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

        for msg in &messages {
            let content = message_to_content(msg.clone());
            builder = match msg.role {
                Role::User => builder.message(Role::User, content),
                Role::Assistant => builder.message(Role::Assistant, content),
            };
        }

        builder = builder.tools(tools);

        let params = builder.build();
        self.client.messages().create_stream(params).await
    }

    async fn test_connection(&self) -> Result<(), AnthropicError> {
        self.client.test_connection().await
    }
}
