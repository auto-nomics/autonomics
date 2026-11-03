use crate::{ContentBlock, Message, ServerToolUsage, StopReason};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum MessageStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: Message },

    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDelta,
        usage: MessageDeltaUsage,
    },

    #[serde(rename = "message_stop")]
    MessageStop,

    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        content_block: ContentBlock,
        index: usize,
    },

    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        delta: ContentBlockDelta,
        index: usize,
    },

    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageDelta {
    pub stop_reason: Option<StopReason>,
    pub stop_sequence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageDeltaUsage {
    pub output_tokens: u64,
    pub input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub server_tool_use: Option<ServerToolUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ContentBlockDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },

    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },

    #[serde(rename = "citations_delta")]
    CitationsDelta { citation: TextCitation },

    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },

    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum TextCitation {
    #[serde(rename = "char_location")]
    CharLocation {
        cited_text: String,
        document_index: usize,
        document_title: Option<String>,
        start_char_index: usize,
        end_char_index: usize,
    },

    #[serde(rename = "page_location")]
    PageLocation {
        cited_text: String,
        document_index: usize,
        document_title: Option<String>,
        start_page_number: usize,
        end_page_number: usize,
    },

    #[serde(rename = "content_block_location")]
    ContentBlockLocation {
        cited_text: String,
        document_index: usize,
        document_title: Option<String>,
        start_block_index: usize,
        end_block_index: usize,
    },

    #[serde(rename = "web_search_result_location")]
    WebSearchResultLocation {
        cited_text: String,
        encrypted_index: String,
        title: Option<String>,
        url: String,
    },
}

pub type MessageStartEvent = MessageStreamEvent;
pub type MessageDeltaEvent = MessageStreamEvent;
pub type MessageStopEvent = MessageStreamEvent;
pub type ContentBlockStartEvent = MessageStreamEvent;
pub type ContentBlockDeltaEvent = MessageStreamEvent;
pub type ContentBlockStopEvent = MessageStreamEvent;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Role, Usage};

    #[test]
    fn test_message_start_event_serialization() {
        let event = MessageStreamEvent::MessageStart {
            message: Message {
                id: "msg_123".to_string(),
                type_: "message".to_string(),
                role: Role::Assistant,
                origin: crate::MessageOrigin::default(),
                content: vec![],
                model: Some("claude-3-5-sonnet-latest".to_string()),
                stop_reason: None,
                stop_sequence: None,
                usage: Some(Usage {
                    input_tokens: 10,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    server_tool_use: None,
                    service_tier: None,
                }),
                request_id: None,
            },
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: MessageStreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn test_content_block_delta_serialization() {
        let event = MessageStreamEvent::ContentBlockDelta {
            delta: ContentBlockDelta::TextDelta {
                text: "Hello".to_string(),
            },
            index: 0,
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: MessageStreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn test_message_delta_event_serialization() {
        let event = MessageStreamEvent::MessageDelta {
            delta: MessageDelta {
                stop_reason: Some(StopReason::EndTurn),
                stop_sequence: None,
            },
            usage: MessageDeltaUsage {
                output_tokens: 25,
                input_tokens: Some(10),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                server_tool_use: None,
            },
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: MessageStreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn test_citation_serialization() {
        let citation = TextCitation::CharLocation {
            cited_text: "Example text".to_string(),
            document_index: 0,
            document_title: Some("Document Title".to_string()),
            start_char_index: 10,
            end_char_index: 23,
        };

        let json = serde_json::to_string(&citation).unwrap();
        let parsed: TextCitation = serde_json::from_str(&json).unwrap();
        assert_eq!(citation, parsed);
    }

    #[test]
    fn test_all_delta_types() {
        let deltas = vec![
            ContentBlockDelta::TextDelta {
                text: "Hello world".to_string(),
            },
            ContentBlockDelta::InputJsonDelta {
                partial_json: r#"{"key": "val"#.to_string(),
            },
            ContentBlockDelta::ThinkingDelta {
                thinking: "Let me think...".to_string(),
            },
            ContentBlockDelta::SignatureDelta {
                signature: "signature_123".to_string(),
            },
        ];

        for delta in deltas {
            let json = serde_json::to_string(&delta).unwrap();
            let parsed: ContentBlockDelta = serde_json::from_str(&json).unwrap();
            assert_eq!(delta, parsed);
        }
    }
}
