use crate::{
    Message, MessageContent, MessageCreateParams, MessageParam, Role, ToolChoice, ToolDefinition,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageBatch {
    pub id: String,
    #[serde(rename = "type")]
    pub object_type: String,
    pub processing_status: BatchStatus,
    pub request_counts: BatchRequestCounts,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub input_file_id: String,
    pub output_file_id: Option<String>,
    pub error_file_id: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    Validating,
    InProgress,
    Finalizing,
    Completed,
    Expired,
    Cancelling,
    Cancelled,
    Failed,
}

impl BatchStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            BatchStatus::Completed
                | BatchStatus::Expired
                | BatchStatus::Cancelled
                | BatchStatus::Failed
        )
    }

    #[must_use]
    pub fn is_processing(&self) -> bool {
        matches!(
            self,
            BatchStatus::Validating | BatchStatus::InProgress | BatchStatus::Finalizing
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRequestCounts {
    pub total: u32,
    pub completed: u32,
    pub failed: u32,
}

impl BatchRequestCounts {
    #[must_use]
    pub fn pending(&self) -> u32 {
        self.total.saturating_sub(self.completed + self.failed)
    }

    #[must_use]
    pub fn completion_percentage(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (f64::from(self.completed) / f64::from(self.total)) * 100.0
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRequest {
    pub custom_id: String,
    pub method: String,
    pub url: String,
    pub body: MessageCreateParams,
}

impl BatchRequest {
    /// Create a [`BatchRequestBuilder`] for constructing a batch request.
    #[must_use]
    pub fn builder(
        custom_id: impl Into<String>,
        model: impl Into<String>,
        max_tokens: u32,
    ) -> BatchRequestBuilder {
        BatchRequestBuilder {
            custom_id: custom_id.into(),
            method: "POST".to_string(),
            url: "/v1/messages".to_string(),
            body: MessageCreateParams {
                model: model.into(),
                max_tokens,
                messages: Vec::new(),
                system: None,
                temperature: None,
                top_p: None,
                top_k: None,
                stop_sequences: None,
                stream: Some(false),
                tools: None,
                tool_choice: None,
                metadata: None,
                thinking: None,
                reasoning: None,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct BatchRequestBuilder {
    custom_id: String,
    method: String,
    url: String,
    body: MessageCreateParams,
}

impl BatchRequestBuilder {
    #[must_use]
    pub fn user(mut self, content: impl Into<String>) -> Self {
        self.body.messages.push(MessageParam {
            role: Role::User,
            content: MessageContent::Text(content.into()),
        });
        self
    }

    #[must_use]
    pub fn assistant(mut self, content: impl Into<String>) -> Self {
        self.body.messages.push(MessageParam {
            role: Role::Assistant,
            content: MessageContent::Text(content.into()),
        });
        self
    }

    #[must_use]
    pub fn system(mut self, system: impl Into<String>) -> Self {
        self.body.system = Some(system.into());
        self
    }

    #[must_use]
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.body.temperature = Some(temperature);
        self
    }

    #[must_use]
    pub fn top_p(mut self, top_p: f32) -> Self {
        self.body.top_p = Some(top_p);
        self
    }

    #[must_use]
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.body.top_k = Some(top_k);
        self
    }

    #[must_use]
    pub fn stop_sequences(mut self, stop_sequences: Vec<String>) -> Self {
        self.body.stop_sequences = Some(stop_sequences);
        self
    }

    #[must_use]
    pub fn tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.body.tools = Some(tools);
        self
    }

    #[must_use]
    pub fn tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.body.tool_choice = Some(tool_choice);
        self
    }

    #[must_use]
    pub fn metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.body.metadata = Some(metadata);
        self
    }

    #[must_use]
    pub fn build(self) -> BatchRequest {
        BatchRequest {
            custom_id: self.custom_id,
            method: self.method,
            url: self.url,
            body: self.body,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    pub custom_id: String,
    pub response: BatchResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResponse {
    pub status_code: u16,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    pub body: BatchResponseBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BatchResponseBody {
    Success(Message),
    Error(BatchError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
    #[serde(default)]
    pub details: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCreateParams {
    pub requests: Vec<BatchRequest>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_window: Option<u32>,
}

impl BatchCreateParams {
    #[must_use]
    pub fn new(requests: Vec<BatchRequest>) -> Self {
        Self {
            requests,
            metadata: HashMap::new(),
            completion_window: None,
        }
    }

    #[must_use]
    pub fn with_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.metadata = metadata;
        self
    }

    #[must_use]
    pub fn with_completion_window(mut self, hours: u32) -> Self {
        self.completion_window = Some(hours);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BatchListParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl BatchListParams {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn after(mut self, after: impl Into<String>) -> Self {
        self.after = Some(after.into());
        self
    }

    #[must_use]
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit.clamp(1, 100));
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchList {
    pub data: Vec<MessageBatch>,
    pub has_more: bool,
    pub first_id: Option<String>,
    pub last_id: Option<String>,
}

impl MessageBatch {
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.processing_status == BatchStatus::Completed
    }

    #[must_use]
    pub fn has_failed(&self) -> bool {
        matches!(
            self.processing_status,
            BatchStatus::Failed | BatchStatus::Expired
        )
    }

    #[must_use]
    pub fn can_cancel(&self) -> bool {
        self.processing_status.is_processing()
    }

    #[must_use]
    pub fn completion_percentage(&self) -> f64 {
        self.request_counts.completion_percentage()
    }

    #[must_use]
    pub fn pending_requests(&self) -> u32 {
        self.request_counts.pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_status_terminal() {
        assert!(BatchStatus::Completed.is_terminal());
        assert!(BatchStatus::Failed.is_terminal());
        assert!(BatchStatus::Cancelled.is_terminal());
        assert!(BatchStatus::Expired.is_terminal());
        assert!(BatchStatus::InProgress.is_processing());
    }

    #[test]
    fn test_batch_request_builder() {
        let request = BatchRequest::builder("test1", "claude-3-5-sonnet-latest", 1024)
            .user("Hello, world!")
            .system("You are a helpful assistant")
            .temperature(0.7)
            .build();

        assert_eq!(request.custom_id, "test1");
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "/v1/messages");
        assert_eq!(request.body.model, "claude-3-5-sonnet-latest");
        assert_eq!(request.body.max_tokens, 1024);
        assert_eq!(request.body.messages.len(), 1);
        assert_eq!(
            request.body.system,
            Some("You are a helpful assistant".to_string())
        );
        assert_eq!(request.body.temperature, Some(0.7));
    }

    #[test]
    fn test_request_counts() {
        let counts = BatchRequestCounts {
            total: 100,
            completed: 75,
            failed: 10,
        };

        assert_eq!(counts.pending(), 15);
        assert_eq!(counts.completion_percentage(), 75.0);
    }

    #[test]
    fn test_batch_create_params() {
        let requests = vec![
            BatchRequest::builder("req1", "claude-3-5-sonnet-latest", 1024)
                .user("Hello")
                .build(),
        ];

        let params = BatchCreateParams::new(requests).with_completion_window(12);

        assert_eq!(params.requests.len(), 1);
        assert_eq!(params.completion_window, Some(12));
    }
}
