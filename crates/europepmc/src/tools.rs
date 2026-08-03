//! Agent tool layer wrapping the Europe PMC SDK.
//!
//! Each tool maps to one or more client methods. Wire them into an agent's
//! toolset via [`europepmc_registrations`].

mod article;
mod citations;
mod references;
mod search;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

pub(crate) use self::helpers::json_err;
use crate::EuropePmcClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::EuropePmcError;

    pub(crate) fn json_err(e: EuropePmcError) -> ToolError {
        ToolError::ExecutionFailed { source: Box::new(e) }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all Europe PMC tools.
///
/// Pass a shared [`EuropePmcClient`] so every tool reuses the same HTTP
/// connection pool.
pub fn europepmc_registrations(client: Arc<EuropePmcClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(search::EuropePmcSearchTool {
            client: client.clone(),
        }),
        R::from(article::EuropePmcArticleTool {
            client: client.clone(),
        }),
        R::from(references::EuropePmcReferencesTool {
            client: client.clone(),
        }),
        R::from(citations::EuropePmcCitationsTool { client }),
    ]
}
