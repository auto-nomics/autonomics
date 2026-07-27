//! Tool framework (trait, registry, executor) and built-in lifecycle tools.
//!
//! - The *framework* modules (`function`, `registry`, `toolset`,
//!   `executor`, `error`, `truncation`) define how tools are declared and dispatched.
//! - [`builtins`] holds the lifecycle tool implementations (attempt_complete, abort_task).
//! - Primitive tools (bash, read, write, edit, glob, grep, webfetch)
//!   live in the separate `agentik-tools` crate.

pub mod builtins;
pub mod error;
pub mod executor;
pub mod function;
pub mod registry;
pub mod task_runtime;
pub mod tool_provider;
pub mod toolset;
pub mod truncation;

pub use error::{ToolError, ToolOperationResult};
pub use executor::{ToolExecutionConfig, ToolExecutionConfigBuilder, ToolExecutor};
pub use function::{DynToolFunction, ToolContext, ToolFunction};
pub use registry::{SharedToolRegistry, ToolRegistry};
pub use tool_provider::ToolProviderRegistry;
pub use toolset::{ToolRegistration, Toolset};

pub use agentik_sdk::types::{
    ToolChoice, ToolDefinition, ToolDefinitionBuilder, ToolResult, ToolResultContent, ToolUse,
    ToolValidationError,
};

// Re-export lifecycle tools at the `tools` facade so callers can do
// `use agentik_core::tools::{AbortTaskTool, ...}`.
pub use builtins::{
    AbortTaskInput, AbortTaskTool, TaskResultViewerTool, ViewTaskResultsInput, WaitTaskInput,
    WaitTaskTool, lifecycle_registrations, task_registrations,
};
