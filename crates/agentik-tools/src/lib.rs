//! Built-in primitive tools for the agentik-core runtime.
//!
//! This crate provides the standard set of filesystem, shell, and web tools
//! that agents can use. These are intentionally kept out of `agentik-core` so
//! that the framework remains dependency-light and can be reused in
//! environments that don't need bash/fs/web capabilities.

pub mod apply_patch;
pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod read;
pub mod webfetch;
pub mod write;

pub use apply_patch::{ApplyPatchInput, ApplyPatchTool};
pub use bash::{BashInput, BashTool};
pub use edit::{EditInput, EditTool};
pub use glob::{GlobInput, GlobTool};
pub use grep::{GrepInput, GrepTool};
pub use read::{ReadInput, ReadTool};
pub use webfetch::{WebFetchInput, WebFetchTool};
pub use write::{WriteInput, WriteTool};

pub use agentik_core::tools::ToolRegistration;

/// The foundational primitive tools: bash, read, write, edit, apply_patch,
/// glob, grep, webfetch. Wire these into an agent's toolset for basic
/// filesystem, command, and web-fetch capability.
pub fn primitive_registrations() -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(BashTool),
        ToolRegistration::from(ReadTool),
        ToolRegistration::from(WriteTool),
        ToolRegistration::from(EditTool),
        ToolRegistration::from(ApplyPatchTool),
        ToolRegistration::from(GlobTool),
        ToolRegistration::from(GrepTool),
        ToolRegistration::from(WebFetchTool),
    ]
}
