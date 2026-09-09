//! Agent tools for RCSB PDB previews and summaries.

mod entry;
mod polymer;
mod search;
mod structure;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

use crate::RcsbClient;

pub(crate) fn tool_error(error: crate::RcsbError) -> agentik_core::tools::ToolError {
    agentik_core::tools::ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}

/// Build registrations for all RCSB tools.
pub fn rcsb_registrations(client: Arc<RcsbClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as Registration;
    vec![
        Registration::from(search::RcsbSearchTool {
            client: client.clone(),
        }),
        Registration::from(entry::RcsbEntryTool {
            client: client.clone(),
        }),
        Registration::from(polymer::RcsbPolymerTool {
            client: client.clone(),
        }),
        Registration::from(structure::RcsbStructurePreviewTool { client }),
    ]
}
