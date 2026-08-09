//! Agent tools for the writing system.
//!
//! Tools expose the document editing, citation management, and compilation
//! pipeline to LLM agents via the `agentik` tool framework.
//!
//! Use [`writing_all_registrations`] to get a `Vec<ToolRegistration>` ready
//! for insertion into a [`ToolRegistry`](agentik_core::tools::ToolRegistry).

pub mod citation_tools;
pub mod compile_tools;
pub mod document_tools;
pub mod edit_tools;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;
use bib_base::BibBase;

use crate::compile::LatexEngine;
use crate::store::WritingStore;

/// Shared state bundle for writing tools.
#[derive(Clone)]
pub struct WritingToolState {
    pub store: Arc<WritingStore>,
    pub bib: Option<Arc<BibBase>>,
    pub engine: Option<Arc<dyn LatexEngine>>,
}

/// Build all writing-system tool registrations.
///
/// Pass `None` for `bib` / `engine` to disable citation / compilation tools.
pub fn writing_all_registrations(
    store: Arc<WritingStore>,
    bib: Option<Arc<BibBase>>,
    engine: Option<Arc<dyn LatexEngine>>,
) -> Vec<ToolRegistration> {
    let state = WritingToolState { store, bib, engine };

    let mut regs = Vec::new();

    // Document management
    regs.extend(document_tools::registrations(state.clone()));
    // Editing
    regs.extend(edit_tools::registrations(state.clone()));
    // Citations (only if bib-base is available)
    regs.extend(citation_tools::registrations(state.clone()));
    // Compilation (only if engine is available)
    regs.extend(compile_tools::registrations(state));

    regs
}
