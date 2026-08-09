//! Agent tool layer wrapping the Crossref SDK.
//!
//! Each tool maps to one or more client methods and returns LLM-friendly
//! Markdown. Wire them into an agent's toolset via
//! [`crossref_registrations`].

pub mod doi;
pub mod search;
pub mod types;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

pub(crate) use self::helpers::json_err;
use crate::CrossrefClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::CrossrefError;

    pub(crate) fn json_err(e: CrossrefError) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all Crossref tools.
///
/// Pass a shared [`CrossrefClient`] so every tool reuses the same HTTP
/// connection pool.
pub fn crossref_registrations(client: Arc<CrossrefClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(search::CrossrefSearchTool {
            client: client.clone(),
        }),
        R::from(doi::CrossrefDoiTool {
            client: client.clone(),
        }),
        R::from(types::CrossrefTypesTool { client }),
    ]
}
