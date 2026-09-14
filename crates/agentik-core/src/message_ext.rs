use agentik_sdk::types::messages::{ContentBlock, Message, Role, StopReason};
use agentik_sdk::types::shared::Usage;
use serde_json::Value;
use uuid::Uuid;

/// Convenience extension trait for constructing `Message` values.
pub trait AgentMessageExt {
    fn system(text: impl Into<String>) -> Self;
    fn user(text: impl Into<String>) -> Self;
    /// User message from arbitrary content blocks (e.g. a text marker
    /// followed by images returned by a tool).
    fn user_blocks(blocks: Vec<ContentBlock>) -> Self;
    fn assistant_text(text: impl Into<String>) -> Self;
    fn assistant_tool_use(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self;
    fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self;
    fn with_usage(self, usage: Usage) -> Self;
    fn with_model(self, model: impl Into<String>) -> Self;
    fn with_stop_reason(self, reason: StopReason) -> Self;
    fn text(&self) -> Vec<String>;
    fn tool_calls(&self) -> Vec<&ContentBlock>;
    fn has_tool_calls(&self) -> bool;
}

impl AgentMessageExt for Message {
    fn system(text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            type_: "message".to_string(),
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        }
    }

    fn user(text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            type_: "message".to_string(),
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        }
    }

    fn user_blocks(blocks: Vec<ContentBlock>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            type_: "message".to_string(),
            role: Role::User,
            content: blocks,
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        }
    }

    fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            type_: "message".to_string(),
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        }
    }

    fn assistant_tool_use(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        let id_str = id.into();
        Self {
            id: id_str.clone(),
            type_: "message".to_string(),
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id_str,
                name: name.into(),
                input,
            }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        }
    }

    fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            type_: "message".to_string(),
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: Some(content.into()),
                is_error: Some(is_error),
            }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        }
    }

    fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    fn with_stop_reason(mut self, reason: StopReason) -> Self {
        self.stop_reason = Some(reason);
        self
    }

    fn text(&self) -> Vec<String> {
        self.content
            .iter()
            .filter_map(|c| {
                if let ContentBlock::Text { text } = c {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    fn tool_calls(&self) -> Vec<&ContentBlock> {
        self.content
            .iter()
            .filter(|c| matches!(c, ContentBlock::ToolUse { .. }))
            .collect()
    }

    fn has_tool_calls(&self) -> bool {
        self.content
            .iter()
            .any(|c| matches!(c, ContentBlock::ToolUse { .. }))
    }
}
