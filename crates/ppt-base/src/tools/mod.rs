//! Agent tools for the deck engine.
//!
//! Exposes deck CRUD, slide operations (the Slidev MCP patch model),
//! asset attachment, and export to LLM agents via the `agentik` tool
//! framework. Use [`deck_all_registrations`] to get a
//! `Vec<ToolRegistration>` for a [`ToolRegistry`](agentik_core::tools::ToolRegistry).

pub mod asset_tools;
pub mod citation_tools;
pub mod deck_tools;
pub mod export_tools;
pub mod slide_tools;

use agentik_core::tools::ToolRegistration;

use crate::actor::DeckHandle;

/// Shared state bundle for deck tools: the write actor handle (reads go
/// through `handle.shared()` directly).
#[derive(Clone)]
pub struct DeckToolState {
    pub handle: DeckHandle,
}

/// Build all deck-engine tool registrations.
pub fn deck_all_registrations(handle: DeckHandle) -> Vec<ToolRegistration> {
    let state = DeckToolState { handle };

    let mut regs = Vec::new();
    regs.extend(deck_tools::registrations(state.clone()));
    regs.extend(slide_tools::registrations(state.clone()));
    regs.extend(asset_tools::registrations(state.clone()));
    regs.extend(export_tools::registrations(state.clone()));
    regs.extend(citation_tools::registrations(state));
    regs
}

/// Bound a 1-based slide number check into a tool error.
pub(crate) fn tool_err(e: crate::Error) -> agentik_core::tools::ToolError {
    agentik_core::tools::ToolError::ExecutionFailed {
        source: Box::new(e),
    }
}

/// Wrap a write closure's future into the boxed form [`DeckHandle::write`]
/// expects, keeping call sites free of pin/impl noise.
pub(crate) fn box_write<T, F>(f: F) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::Result<T>> + Send>>
where
    T: Send + 'static,
    F: std::future::Future<Output = crate::Result<T>> + Send + 'static,
{
    Box::pin(f)
}

pub(crate) use crate::error as deck_error;
