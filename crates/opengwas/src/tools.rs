//! Agent tool layer wrapping the OpenGWAS SDK.
//!
//! Each tool maps to one or more SDK methods. Wire them into an agent's
//! toolset via [`opengwas_registrations`].

mod associations;
pub mod download;
mod gwasinfo_by_id;
mod gwasinfo_count;
mod gwasinfo_search;
mod ld_clump;
mod ld_matrix;
mod phewas;
mod variants_chrpos;
mod variants_rsid;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;
use fs::OpendalFileStorage;

pub(crate) use self::helpers::json_err;
use crate::OpengwasClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

mod helpers {
    use agentik_core::tools::ToolError;

    use crate::error::OpengwasError;

    pub(crate) fn json_err(e: OpengwasError) -> ToolError {
        ToolError::ExecutionFailed {
            source: Box::new(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all OpenGWAS tools.
///
/// Pass a shared [`OpengwasClient`] so every tool reuses the same HTTP
/// connection and SQLite cache, and a shared [`OpendalFileStorage`] for
/// file download operations.
pub fn opengwas_registrations(
    client: Arc<OpengwasClient>,
    storage: Arc<OpendalFileStorage>,
) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(gwasinfo_by_id::GwasinfoByIdTool {
            client: client.clone(),
        }),
        R::from(gwasinfo_search::GwasinfoSearchTool {
            client: client.clone(),
        }),
        R::from(gwasinfo_count::GwasinfoCountTool {
            client: client.clone(),
        }),
        R::from(associations::AssociationsTool {
            client: client.clone(),
        }),
        R::from(phewas::PhewasTool {
            client: client.clone(),
        }),
        R::from(variants_rsid::VariantsRsidTool {
            client: client.clone(),
        }),
        R::from(variants_chrpos::VariantsChrposTool {
            client: client.clone(),
        }),
        R::from(ld_clump::LdClumpTool {
            client: client.clone(),
        }),
        R::from(ld_matrix::LdMatrixTool {
            client: client.clone(),
        }),
        R::from(download::DownloadFilesTool::new(client, storage)),
    ]
}
