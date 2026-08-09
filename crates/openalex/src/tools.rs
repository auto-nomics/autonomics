//! Agent tool layer wrapping the OpenAlex SDK.
//!
//! Each tool maps to one client method. Wire them into an agent's
//! toolset via [`openalex_registrations`].

mod autocomplete;
mod get_work;
mod search;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

pub(crate) use self::helpers::json_err;
use crate::OpenAlexClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::OpenAlexError;

    pub(crate) fn json_err(e: OpenAlexError) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all OpenAlex tools.
///
/// Pass a shared [`OpenAlexClient`] so every tool reuses the same HTTP
/// connection pool.
pub fn openalex_registrations(client: Arc<OpenAlexClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(search::OpenAlexSearchTool {
            client: client.clone(),
        }),
        R::from(get_work::OpenAlexGetWorkTool {
            client: client.clone(),
        }),
        R::from(autocomplete::OpenAlexAutocompleteTool { client }),
    ]
}

/// Build [`ToolRegistration`]s for OpenAlex tools whose capabilities are
/// **not** covered by the [`bib_base::LiteratureGateway`].
///
/// The gateway already provides unified `search` and `fetch` for works, so
/// this function registers only the cross-entity `autocomplete` tool. Use
/// it alongside the gateway to expose OpenAlex's full surface area without
/// duplicating search/fetch.
pub fn openalex_extended_registrations(client: Arc<OpenAlexClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![R::from(autocomplete::OpenAlexAutocompleteTool { client })]
}
