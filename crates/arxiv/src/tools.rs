//! Agent tool layer wrapping the arXiv SDK.
//!
//! Each tool maps to one or more arXiv client methods. Wire them into an
//! agent's toolset via [`arxiv_registrations`].

mod fetch;
mod search;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

pub(crate) use self::helpers::json_err;
use crate::ArxivClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::ArxivError;

    pub(crate) fn json_err(e: ArxivError) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all arXiv tools.
///
/// Pass a shared [`ArxivClient`] so every tool reuses the same HTTP
/// connection pool and rate-limit state.
pub fn arxiv_registrations(client: Arc<ArxivClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(search::ArxivSearchTool {
            client: client.clone(),
        }),
        R::from(fetch::ArxivFetchTool { client }),
    ]
}
