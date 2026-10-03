//! Tool framework (trait, registry, executor) and built-in tools.
//!
//! - The *framework* modules (`function`, `toolset`, `error`, `truncation`)
//!   define how tools are declared and dispatched.
//! - [`builtins`] holds the plan and task tool implementations.

pub mod builtins;
pub mod error;
pub mod function;
pub mod task_runtime;
pub mod toolset;
pub mod truncation;

pub use error::{ToolError, ToolOperationResult};
pub use function::{
    DynToolFunction, ExecutionMode, MAX_PROGRESS_RECORDS, ProgressBuffer, ProgressLog,
    ProgressRecord, TaskMetadata, ToolContext, ToolFunction,
};
pub use task_runtime::TaskStore;
pub use toolset::{ToolRegistration, ToolRegistry, Toolset};

pub use agentik_sdk::types::{
    ToolChoice, ToolDefinition, ToolDefinitionBuilder, ToolResult, ToolResultContent, ToolUse,
    ToolValidationError,
};

// Re-export built-in tools at the `tools` facade so callers can do
// `use agentik_core::tools::{WaitTaskTool, ...}`.
pub use builtins::{
    PlanHandle, PlanStepInput, TaskResultViewerTool, UpdatePlanInput, UpdatePlanTool,
    ViewTaskResultsInput, WaitTaskInput, WaitTaskTool, plan_registrations, task_registrations,
};
