use agentik_sdk::types::messages::{ContentBlock, Message, Role};
use uuid::Uuid;

/// Constructors needed by the standalone memory pipeline.
pub trait MemoryMessageExt {
    fn user(text: impl Into<String>) -> Self;
    fn assistant_text(text: impl Into<String>) -> Self;
}

impl MemoryMessageExt for Message {
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
}
