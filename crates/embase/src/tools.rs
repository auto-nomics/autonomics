//! Agent tool layer wrapping the Embase SDK.
//!
//! Each tool maps to one or more Embase client methods. Wire them into an
//! agent's toolset via [`embase_registrations`].

mod retrieve;
mod search;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

pub(crate) use self::helpers::json_err;
use crate::EmbaseClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::EmbaseError;

    pub(crate) fn json_err(e: EmbaseError) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all Embase tools.
///
/// Pass a shared [`EmbaseClient`] so every tool reuses the same HTTP
/// connection pool.
pub fn embase_registrations(client: Arc<EmbaseClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(search::EmbaseSearchTool {
            client: client.clone(),
        }),
        R::from(retrieve::EmbaseRetrieveTool { client }),
    ]
}
