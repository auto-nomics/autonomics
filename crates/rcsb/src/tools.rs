//! Agent tools for RCSB PDB previews and summaries.
//!
//! The table surfaces (`rcsb_search`, `rcsb_entry`, `rcsb_polymer`) have
//! been deregistered in favor of the DAG nodes `source_rcsb_search` /
//! `source_rcsb_entry` / `source_rcsb_polymer_entity` (and
//! `source_rcsb_structure` / `source_rcsb_assembly` for full files). The
//! one tool left here is the human-facing half: a bounded mmCIF/PDB/FASTA
//! preview for quick reading inside a conversation.

mod structure;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

use crate::RcsbClient;

pub(crate) fn tool_error(error: crate::RcsbError) -> agentik_core::tools::ToolError {
    agentik_core::tools::ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}

/// Build registrations for the remaining RCSB tool.
pub fn rcsb_registrations(client: Arc<RcsbClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as Registration;
    vec![Registration::from(structure::RcsbStructurePreviewTool {
        client,
    })]
}
