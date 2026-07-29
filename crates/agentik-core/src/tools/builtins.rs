//! Built-in lifecycle and task tools.
//!
//! These are the always-injected tools that the agent framework requires
//! for task signaling. Primitive tools (bash, read, write, etc.) live in
//! the `agentik-tools` crate.

pub mod lifecycle;
pub mod task_tools;

pub use lifecycle::lifecycle_registrations;
pub use task_tools::{
    TaskResultViewerTool, ViewTaskResultsInput, WaitTaskInput, WaitTaskTool, task_registrations,
};
