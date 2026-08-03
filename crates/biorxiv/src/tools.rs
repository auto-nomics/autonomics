//! Agent tool layer wrapping the bioRxiv/medRxiv SDK.
//!
//! Each tool maps to one or more client methods. Wire them into an
//! agent's toolset via [`biorxiv_registrations`].

mod details;
mod search;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

pub(crate) use self::helpers::json_err;
use crate::BiorxivClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::BiorxivError;

    pub(crate) fn json_err(e: BiorxivError) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all bioRxiv/medRxiv tools.
///
/// Pass a shared [`BiorxivClient`] so every tool reuses the same HTTP
/// connection pool.
pub fn biorxiv_registrations(client: Arc<BiorxivClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(details::BiorxivDetailsTool {
            client: client.clone(),
        }),
        R::from(search::BiorxivSearchTool { client }),
    ]
}
