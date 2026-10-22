use agentik_sdk::ToolResult;

pub type ToolRunResult = std::result::Result<ToolResult, ToolError>;

/// Tool execution errors.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// Tool not found in registry.
    #[error("Tool '{name}' not found")]
    NotFound { name: String },

    /// Tool validation failed.
    #[error("Tool validation failed: {message}")]
    ValidationFailed { message: String },

    /// Tool execution failed.
    #[error("Tool execution failed: {source}")]
    ExecutionFailed {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Tool execution timed out.
    #[error("Tool execution timed out after {seconds} seconds")]
    Timeout { seconds: u64 },

    #[error("Tool execution cancelled")]
    Cancel,

    /// Tool registry error.
    #[error("Tool registry error: {message}")]
    RegistryError { message: String },
}

impl Clone for ToolError {
    fn clone(&self) -> Self {
        match self {
            Self::NotFound { name } => Self::NotFound { name: name.clone() },
            Self::ValidationFailed { message } => Self::ValidationFailed {
                message: message.clone(),
            },
            Self::ExecutionFailed { source } => Self::ExecutionFailed {
                source: Box::new(ErrorMessage(source.to_string())),
            },
            Self::Timeout { seconds } => Self::Timeout { seconds: *seconds },
            Self::Cancel => Self::Cancel,
            Self::RegistryError { message } => Self::RegistryError {
                message: message.clone(),
            },
        }
    }
}

/// A lightweight, cloneable error that preserves the display message of the
/// original error. Used by [`ToolError::clone`] because trait objects are not
/// cloneable.
#[derive(Debug, Clone)]
struct ErrorMessage(String);

impl std::fmt::Display for ErrorMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ErrorMessage {}

impl From<&str> for ToolError {
    fn from(message: &str) -> Self {
        ToolError::ValidationFailed {
            message: message.to_string(),
        }
    }
}

impl From<String> for ToolError {
    fn from(message: String) -> Self {
        ToolError::ValidationFailed { message }
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(e: serde_json::Error) -> Self {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

/// Result type for tool operations.
pub type ToolOperationResult<T> = Result<T, ToolError>;
