//! Agent tool layer wrapping the GWAS Catalog SDK.
//!
//! Wire into an agent's toolset via [`gwascatalog_registrations`].

mod associations;
mod efo_traits;
mod search;
mod snp;
mod studies;
mod study_associations;
mod summary_associations;
mod summary_variant;
mod unpublished;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;

pub(crate) use self::helpers::json_err;
use crate::client::GwasCatalogClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::GwasCatalogError;

    pub(crate) fn json_err(e: GwasCatalogError) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all GWAS Catalog tools.
///
/// Pass a shared [`GwasCatalogClient`] so every tool reuses the same HTTP
/// connection pool.
pub fn gwascatalog_registrations(client: Arc<GwasCatalogClient>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(search::SearchTool {
            client: client.clone(),
        }),
        R::from(studies::StudiesTool {
            client: client.clone(),
        }),
        R::from(study_associations::StudyAssociationsTool {
            client: client.clone(),
        }),
        R::from(associations::AssociationsTool {
            client: client.clone(),
        }),
        R::from(snp::SnpTool {
            client: client.clone(),
        }),
        R::from(efo_traits::EfoTraitsTool {
            client: client.clone(),
        }),
        R::from(unpublished::UnpublishedTool {
            client: client.clone(),
        }),
        R::from(summary_associations::SummaryAssociationsTool {
            client: client.clone(),
        }),
        R::from(summary_variant::SummaryVariantTool { client }),
    ]
}
