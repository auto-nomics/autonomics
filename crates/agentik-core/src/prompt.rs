//! System-prompt construction — the single home for prompt assembly.
//!
//! Layers:
//! - [`capabilities`]: [`PromptCapabilities`] gates which tool-group sections
//!   an agent's prompt includes (implemented by `AgentKind` here and by the
//!   runtime's `RuntimeConfig`).
//! - [`sections`]: the domain section constants and
//!   [`sections::build_system_prompt`], the capability-gated dynamic builder.
//! - [`system_prompt_builder`]: `SystemPromptBuilder`, the layered assembler
//!   (identity → extras → skill → wiki → tool-use guidance) used by the agent
//!   loop each turn.
//! - [`compact`] / [`prompt_context`]: compaction and dynamic prompt context.

pub mod compact;
pub mod prompt_context;
pub mod system_prompt_builder;

mod capabilities;
mod sections;

pub use capabilities::PromptCapabilities;
pub use sections::{build_system_prompt, default_system_prompt};
