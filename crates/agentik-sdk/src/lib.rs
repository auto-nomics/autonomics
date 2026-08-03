pub mod client;
pub mod config;
pub mod files;
pub mod http;
pub mod model;
pub mod provider;
pub mod resources;
pub mod streaming;
pub mod tokens;
pub mod wire;
pub mod types {
    pub use agentik_types::*;
}

pub use agentik_types;

pub use agentik_types::{
    AnthropicError, BatchCreateParams, BatchError, BatchList, BatchListParams, BatchRequest,
    BatchRequestBuilder, BatchRequestCounts, BatchResponse, BatchResponseBody, BatchResult,
    BatchStatus, ContentBlock, ContentBlockDelta, ContentBlockParam, FileDownload, FileList,
    FileListParams, FileObject, FileOrder, FilePurpose, FileStatus, FileUploadParams, ImageSource,
    Message, MessageBatch, MessageContent, MessageCreateBuilder, MessageCreateParams, MessageDelta,
    MessageDeltaUsage, MessageParam, MessageStreamEvent, ModelList, ModelListParams, ModelObject,
    ReasoningConfig, ReasoningEffort, RequestId, Result, Role, ServerTool, StopReason, StorageInfo,
    TextCitation, ThinkingConfig, ThinkingKind, ToolChoice, ToolDefinition, ToolDefinitionBuilder,
    ToolResult, ToolResultContent, ToolUse, ToolValidationError, UploadProgress, Usage,
    WebSearchParameters, anthropic_budget_for_effort,
};
pub use client::Anthropic;
pub use config::{ClientConfig, LogLevel};
pub use files::{File, FileBuilder, FileConstraints, FileData, FileError, FileSource, to_file};
pub use http::auth::AuthMethod;
pub use http::{RetryCondition, RetryExecutor, RetryPolicy, RetryResult, api_retry, default_retry};
pub use resources::{BatchesResource, FilesResource, MessagesResource, ModelsResource};
pub use streaming::MessageStream;
pub use tokens::{ModelPrice, ModelUsage, RequestUsage, TokenCounter, UsageStats, UsageSummary};
pub use wire::{
    AnthropicWire, OpenAiChatWire, OpenAiResponsesWire, ProtocolFeatures, StreamState,
    ThinkingSupport, WireProtocol, WireProtocolKind, build_wire, wire_protocol_for_provider,
};

pub trait ContentBlockParamExt {
    fn image_file(
        file: File,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<Self, FileError>> + Send>,
    >
    where
        Self: Sized;
}

impl ContentBlockParamExt for ContentBlockParam {
    fn image_file(
        file: File,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<Self, FileError>> + Send>,
    > {
        Box::pin(async move {
            if !file.is_image() {
                return Err(FileError::InvalidMimeType {
                    mime_type: file.mime_type.to_string(),
                    allowed: vec!["image/*".to_string()],
                });
            }
            let base64_data = file.to_base64().await?;
            Ok(ContentBlockParam::Image {
                source: ImageSource::Base64 {
                    media_type: file.mime_type.to_string(),
                    data: base64_data,
                },
            })
        })
    }
}

pub trait FileDownloadExt {
    fn save_to_file(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send + '_>>;
}

impl FileDownloadExt for FileDownload {
    fn save_to_file(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send + '_>> {
        let path = path.as_ref().to_path_buf();
        Box::pin(async move { tokio::fs::write(path, &self.content).await })
    }
}
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const USER_AGENT: &str = concat!("agentik-sdk/", env!("CARGO_PKG_VERSION"));

pub type Error = AnthropicError;
pub use agentik_types as types_module;
