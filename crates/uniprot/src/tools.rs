//! Agent tool layer wrapping the UniProt SDK.
//!
//! Each tool maps to one client method. Wire them into an agent's toolset
//! via [`uniprot_registrations`].

mod entry;
mod idmap;
mod proteomes;
mod search;
mod taxonomy;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

pub(crate) use self::helpers::json_err;
use crate::UniProtClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::UniProtError;

    pub(crate) fn json_err(e: UniProtError) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all UniProt tools.
///
/// Pass a shared [`UniProtClient`] so every tool reuses the same HTTP
/// connection pool.
pub fn uniprot_registrations(client: Arc<UniProtClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(search::UniprotSearchTool {
            client: client.clone(),
        }),
        R::from(entry::UniprotEntryTool {
            client: client.clone(),
        }),
        R::from(idmap::UniprotIdmapTool {
            client: client.clone(),
        }),
        R::from(taxonomy::UniprotTaxonomyTool {
            client: client.clone(),
        }),
        R::from(proteomes::UniprotProteomesTool { client }),
    ]
}
