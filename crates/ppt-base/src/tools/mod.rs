//! Agent tools for the deck engine.
//!
//! Exposes deck CRUD, slide operations (the Slidev MCP patch model),
//! asset attachment, and export to LLM agents via the `agentik` tool
//! framework. Use [`deck_all_registrations`] to get a
//! `Vec<ToolRegistration>` for a [`ToolRegistry`](agentik_core::tools::ToolRegistry).

pub mod asset_tools;
pub mod deck_tools;
pub mod export_tools;
pub mod slide_tools;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

use crate::export::DeckExporter;
use crate::shared::DeckShared;
use crate::store::DeckStore;

/// Shared state bundle for deck tools.
#[derive(Clone)]
pub struct DeckToolState {
    pub store: Arc<DeckStore>,
    pub shared: DeckShared,
}

/// Build all deck-engine tool registrations.
pub fn deck_all_registrations(shared: DeckShared) -> Vec<ToolRegistration> {
    let state = DeckToolState {
        store: shared.store.clone(),
        shared,
    };

    let mut regs = Vec::new();
    regs.extend(deck_tools::registrations(state.clone()));
    regs.extend(slide_tools::registrations(state.clone()));
    regs.extend(asset_tools::registrations(state.clone()));
    regs.extend(export_tools::registrations(state));
    regs
}

/// Bound a 1-based slide number check into a tool error.
pub(crate) fn tool_err(e: crate::Error) -> agentik_core::tools::ToolError {
    agentik_core::tools::ToolError::ExecutionFailed {
        source: Box::new(e),
    }
}

pub(crate) use crate::error as deck_error;
