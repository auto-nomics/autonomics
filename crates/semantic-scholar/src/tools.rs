//! Agent tool layer wrapping the Semantic Scholar SDK.
//!
//! Each tool maps to one or more client methods. Wire them into an agent's
//! toolset via [`s2_registrations`].

mod author;
mod citations;
mod paper;
mod recommendations;
mod references;
mod search;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

pub(crate) use self::helpers::json_err;
use crate::S2Client;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::S2Error;

    pub(crate) fn json_err(e: S2Error) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all Semantic Scholar tools.
///
/// Pass a shared [`S2Client`] so every tool reuses the same HTTP
/// connection pool.
pub fn s2_registrations(client: Arc<S2Client>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(search::S2SearchTool {
            client: client.clone(),
        }),
        R::from(paper::S2PaperTool {
            client: client.clone(),
        }),
        R::from(citations::S2CitationsTool {
            client: client.clone(),
        }),
        R::from(references::S2ReferencesTool {
            client: client.clone(),
        }),
        R::from(author::S2AuthorTool {
            client: client.clone(),
        }),
        R::from(recommendations::S2RecommendationsTool { client }),
    ]
}
