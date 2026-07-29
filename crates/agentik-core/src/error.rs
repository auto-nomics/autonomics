use thiserror::Error;

use agentik_sdk::types::errors::AnthropicError;
use agentik_sdk::types::tools::ToolUse;

use crate::tools::ToolError;

use crate::memory::error::Error as MemoryError;

pub trait Retryable {
    fn is_retryable(&self) -> bool;
    fn retry_message(&self) -> String;
}

impl Retryable for AnthropicError {
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimit { .. }
                | Self::InternalServer { .. }
                | Self::Connection { .. }
                | Self::ConnectionTimeout
                | Self::StreamError(_)
                | Self::Timeout
                | Self::NetworkError(_)
                | Self::ServiceUnavailable { .. }
        )
    }

    fn retry_message(&self) -> String {
        format!("The previous API request failed: {self}. Please retry.")
    }
}

impl Retryable for ToolError {
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::NotFound { .. }
                | Self::ValidationFailed { .. }
                | Self::ExecutionFailed { .. }
                | Self::Timeout { .. }
        )
    }

    fn retry_message(&self) -> String {
        format!("A tool execution failed: {self}. Please retry with corrected parameters.")
    }
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("ApiClient request error: {0}")]
    ApiRequestError(#[from] AnthropicError),

    #[error("Memory error occurred")]
    MemoryError(#[from] MemoryError),

    #[error("Unknown tool requested:  {0:?}. Existed tools: {1:?}")]
    UnknownTool(Vec<ToolUse>, Vec<agentik_sdk::types::ToolDefinition>),

    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),

    #[error("Max iterations ({0}) reached")]
    MaxIterations(usize),

    #[error("workflow failed at iteration {iteration}: {error}")]
    WorkflowFailed {
        iteration: usize,
        #[source]
        error: Box<AgentError>,
    },

    #[error("missing required config: {0}")]
    MissingConfig(String),

    /// Internal signal: compaction completed and the turn should be rebuilt
    /// with fresh context. Handled by the agent loop — not a user-visible error.
    #[error("compaction rebuild")]
    CompactionRebuild,

    /// The agent loop was interrupted by an external cancellation signal.
    #[error("agent cancelled by user")]
    Cancelled,
}

impl Retryable for AgentError {
    fn is_retryable(&self) -> bool {
        match self {
            Self::ApiRequestError(e) => e.is_retryable(),
            Self::Tool(e) => e.is_retryable(),
            _ => false,
        }
    }

    fn retry_message(&self) -> String {
        match self {
            Self::ApiRequestError(e) => e.retry_message(),
            Self::Tool(e) => e.retry_message(),
            _ => format!("An error occurred: {self}."),
        }
    }
}
