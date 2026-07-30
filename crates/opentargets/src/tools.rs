//! Agent tool layer wrapping the Open Targets Platform SDK.
//!
//! Each tool maps to one SDK method and renders the result as LLM-friendly
//! Markdown. Wire them into an agent's toolset via [`opentargets_registrations`].

pub mod associated_diseases;
pub mod associated_targets;
pub mod disease;
pub mod drug;
pub mod search;
pub mod study;
pub mod target;
pub mod variant;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

pub(crate) use self::helpers::json_err;
use crate::OpenTargetsClient;

// Re-export Pagination so individual tool modules can reference it via `super`.
pub(crate) use crate::Pagination;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::OpenTargetsError;

    pub(crate) fn json_err(e: OpenTargetsError) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all Open Targets tools.
///
/// Pass a shared [`OpenTargetsClient`] so every tool reuses the same HTTP
/// connection pool.
pub fn opentargets_registrations(client: Arc<OpenTargetsClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(search::SearchTool {
            client: client.clone(),
        }),
        R::from(target::TargetTool {
            client: client.clone(),
        }),
        R::from(disease::DiseaseTool {
            client: client.clone(),
        }),
        R::from(associated_diseases::AssociatedDiseasesTool {
            client: client.clone(),
        }),
        R::from(associated_targets::AssociatedTargetsTool {
            client: client.clone(),
        }),
        R::from(drug::DrugTool {
            client: client.clone(),
        }),
        R::from(study::StudyTool {
            client: client.clone(),
        }),
        R::from(variant::VariantTool { client }),
    ]
}
